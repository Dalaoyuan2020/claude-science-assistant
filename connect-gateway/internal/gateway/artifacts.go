package gateway

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
	"github.com/google/uuid"
	_ "github.com/ncruces/go-sqlite3/driver"
)

const maxOutboundArtifactBytes = 20 * 1024 * 1024
const maxOutboundArtifacts = 4

type ArtifactResolver interface {
	Resolve(context.Context, string, int64) (model.OutboundAttachment, error)
}

type claudeScienceArtifactResolver struct {
	databasePath string
	artifactRoot string
}

func defaultArtifactResolver() ArtifactResolver {
	home, err := os.UserHomeDir()
	if err != nil {
		return unavailableArtifactResolver{err: err}
	}
	root := filepath.Join(home, ".claude-science")
	return &claudeScienceArtifactResolver{
		databasePath: filepath.Join(root, "operon-cli.db"),
		artifactRoot: filepath.Join(root, "artifacts"),
	}
}

type unavailableArtifactResolver struct{ err error }

func (resolver unavailableArtifactResolver) Resolve(context.Context, string, int64) (model.OutboundAttachment, error) {
	return model.OutboundAttachment{}, fmt.Errorf("Claude Science artifact storage is unavailable: %w", resolver.err)
}

func (resolver *claudeScienceArtifactResolver) Resolve(ctx context.Context, reference string, notBefore int64) (model.OutboundAttachment, error) {
	reference = strings.TrimSpace(reference)
	if _, err := uuid.Parse(reference); err != nil {
		return model.OutboundAttachment{}, errors.New("artifact reference is invalid")
	}
	database, err := sql.Open("sqlite3", "file:"+filepath.ToSlash(resolver.databasePath)+"?mode=ro")
	if err != nil {
		return model.OutboundAttachment{}, errors.New("open Claude Science artifact index failed")
	}
	defer database.Close()

	var artifactID, versionID, fileName, mimeType, checksum, storagePath string
	var sizeBytes, createdAt int64
	err = database.QueryRowContext(ctx, `
SELECT a.id, v.id, a.filename, v.content_type, v.size_bytes, v.checksum,
       v.storage_path, a.created_at
FROM artifacts AS a
JOIN artifact_versions AS v ON v.artifact_id = a.id
WHERE (v.id = ?) OR (a.id = ? AND v.id = a.latest_version_id)
ORDER BY CASE WHEN v.id = ? THEN 0 ELSE 1 END
LIMIT 1`, reference, reference, reference).Scan(
		&artifactID, &versionID, &fileName, &mimeType, &sizeBytes, &checksum, &storagePath, &createdAt,
	)
	if errors.Is(err, sql.ErrNoRows) {
		return model.OutboundAttachment{}, errors.New("artifact is not present in Claude Science")
	}
	if err != nil {
		return model.OutboundAttachment{}, errors.New("read Claude Science artifact index failed")
	}
	if createdAt < notBefore {
		return model.OutboundAttachment{}, errors.New("artifact predates the current remote request")
	}
	if sizeBytes <= 0 || sizeBytes > maxOutboundArtifactBytes {
		return model.OutboundAttachment{}, errors.New("artifact image is empty or too large")
	}
	if mimeType != "image/jpeg" && mimeType != "image/png" && mimeType != "image/webp" {
		return model.OutboundAttachment{}, errors.New("artifact MIME type is not allowed")
	}
	if len(checksum) != sha256.Size*2 {
		return model.OutboundAttachment{}, errors.New("artifact checksum is invalid")
	}

	root, err := filepath.EvalSymlinks(filepath.Clean(resolver.artifactRoot))
	if err != nil {
		return model.OutboundAttachment{}, errors.New("Claude Science artifact root is unavailable")
	}
	if filepath.IsAbs(storagePath) || filepath.Clean(storagePath) == "." {
		return model.OutboundAttachment{}, errors.New("artifact storage path is invalid")
	}
	fullPath, err := filepath.EvalSymlinks(filepath.Join(root, filepath.Clean(storagePath)))
	if err != nil {
		return model.OutboundAttachment{}, errors.New("artifact file is unavailable")
	}
	relative, err := filepath.Rel(root, fullPath)
	if err != nil || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
		return model.OutboundAttachment{}, errors.New("artifact escaped the managed storage root")
	}
	info, err := os.Stat(fullPath)
	if err != nil || !info.Mode().IsRegular() || info.Size() != sizeBytes {
		return model.OutboundAttachment{}, errors.New("artifact file metadata does not match the index")
	}
	file, err := os.Open(fullPath)
	if err != nil {
		return model.OutboundAttachment{}, errors.New("open artifact image failed")
	}
	data, readErr := io.ReadAll(io.LimitReader(file, maxOutboundArtifactBytes+1))
	closeErr := file.Close()
	if readErr != nil || closeErr != nil || int64(len(data)) != sizeBytes {
		return model.OutboundAttachment{}, errors.New("read artifact image failed")
	}
	detectedMIME, ok := detectOutboundImageType(data)
	if !ok || detectedMIME != mimeType {
		return model.OutboundAttachment{}, errors.New("artifact image signature does not match its MIME type")
	}
	digest := sha256.Sum256(data)
	actualChecksum := hex.EncodeToString(digest[:])
	if !strings.EqualFold(actualChecksum, checksum) {
		return model.OutboundAttachment{}, errors.New("artifact checksum does not match")
	}

	return model.OutboundAttachment{
		ArtifactID: versionID,
		MIMEType:   mimeType,
		FileName:   safeAttachmentName(fileName, artifactID+extensionForMIME(mimeType)),
		SizeBytes:  sizeBytes,
		SHA256:     actualChecksum,
		Data:       data,
	}, nil
}

func detectOutboundImageType(data []byte) (string, bool) {
	if len(data) >= 3 && data[0] == 0xff && data[1] == 0xd8 && data[2] == 0xff {
		return "image/jpeg", true
	}
	if len(data) >= 8 && string(data[:8]) == "\x89PNG\r\n\x1a\n" {
		return "image/png", true
	}
	if len(data) >= 12 && string(data[:4]) == "RIFF" && string(data[8:12]) == "WEBP" {
		return "image/webp", true
	}
	return "", false
}

func extensionForMIME(mimeType string) string {
	switch mimeType {
	case "image/jpeg":
		return ".jpg"
	case "image/png":
		return ".png"
	case "image/webp":
		return ".webp"
	default:
		return ".bin"
	}
}
