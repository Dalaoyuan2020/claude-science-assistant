package mcpserver

import (
	"context"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"path/filepath"
	"strings"

	"github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/gateway"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/model"
)

type emptyArgs struct{}

type listPendingArgs struct {
	WorkspacePath string `json:"workspacePath" jsonschema:"absolute workspace path currently open in Claude Science"`
	Limit         int    `json:"limit,omitempty" jsonschema:"maximum messages to return, from 1 to 100"`
}

type claimArgs struct {
	MessageID     string `json:"messageId" jsonschema:"queued CSA Connect message ID"`
	WorkspacePath string `json:"workspacePath" jsonschema:"absolute workspace path currently open in Claude Science"`
}

type replyArgs struct {
	MessageID string `json:"messageId" jsonschema:"claimed CSA Connect message ID"`
	Text      string `json:"text" jsonschema:"answer to send to the paired external chat"`
	Status    string `json:"status,omitempty" jsonschema:"replied or needs_local_approval"`
}

type progressArgs struct {
	MessageID string `json:"messageId" jsonschema:"claimed CSA Connect message ID"`
	Text      string `json:"text" jsonschema:"cumulative answer snapshot to show in the external chat"`
	Sequence  int    `json:"sequence" jsonschema:"monotonically increasing update sequence starting at 1"`
	Final     bool   `json:"final,omitempty" jsonschema:"true only for the final answer snapshot"`
	Status    string `json:"status,omitempty" jsonschema:"replied or needs_local_approval; used only when final is true"`
}

func NewHandler(connectGateway *gateway.Gateway, token string) http.Handler {
	server := mcp.NewServer(&mcp.Implementation{
		Name:    "csa-connect",
		Version: "1.0.0",
	}, nil)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "connect_get_status",
		Description: "Read CSA Connect queue status. This never executes host commands.",
	}, func(ctx context.Context, request *mcp.CallToolRequest, args emptyArgs) (*mcp.CallToolResult, any, error) {
		counts, err := connectGateway.Store().Counts(ctx)
		if err != nil {
			return nil, nil, err
		}
		routes, err := connectGateway.Store().ListRoutes(ctx)
		if err != nil {
			return nil, nil, err
		}
		return jsonResult(map[string]any{
			"counts":       counts,
			"boundRoutes":  countBoundRoutes(routes),
			"capabilities": map[string]bool{"mcpQueue": true, "workspaceFiles": true, "nativeInject": false},
		})
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "connect_list_pending",
		Description: "List queued messages for the current Claude Science workspace, oldest first.",
	}, func(ctx context.Context, request *mcp.CallToolRequest, args listPendingArgs) (*mcp.CallToolResult, any, error) {
		if !filepath.IsAbs(args.WorkspacePath) {
			return nil, nil, errors.New("workspacePath must be absolute")
		}
		messages, err := connectGateway.ListPending(ctx, args.WorkspacePath, args.Limit)
		if err != nil {
			return nil, nil, err
		}
		return jsonResult(map[string]any{"messages": messages})
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "connect_claim_message",
		Description: "Claim one queued message for this workspace before answering it.",
	}, func(ctx context.Context, request *mcp.CallToolRequest, args claimArgs) (*mcp.CallToolResult, any, error) {
		message, err := connectGateway.Claim(ctx, strings.TrimSpace(args.MessageID), args.WorkspacePath)
		if err != nil {
			return nil, nil, err
		}
		return jsonResult(map[string]any{"message": message})
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "connect_send_reply",
		Description: "Send a text response to the paired chat. Use needs_local_approval when the request requires installation, download, system mutation, or another locally approved action.",
	}, func(ctx context.Context, request *mcp.CallToolRequest, args replyArgs) (*mcp.CallToolResult, any, error) {
		status := strings.TrimSpace(args.Status)
		if status == "" {
			status = model.StatusReplied
		}
		if err := connectGateway.SendReply(ctx, strings.TrimSpace(args.MessageID), args.Text, status); err != nil {
			return nil, nil, err
		}
		return jsonResult(map[string]any{"sent": true, "messageId": args.MessageID, "status": status})
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "connect_send_progress",
		Description: "Update one external chat reply in paragraph-sized cumulative snapshots. Start sequence at 1, increase it for each update, and set final=true on the last snapshot.",
	}, func(ctx context.Context, request *mcp.CallToolRequest, args progressArgs) (*mcp.CallToolResult, any, error) {
		updated, err := connectGateway.SendProgress(ctx, strings.TrimSpace(args.MessageID), args.Text, args.Sequence, args.Final, strings.TrimSpace(args.Status))
		if err != nil {
			return nil, nil, err
		}
		return jsonResult(map[string]any{
			"updated":   updated,
			"messageId": args.MessageID,
			"sequence":  args.Sequence,
			"final":     args.Final,
		})
	})

	streamable := mcp.NewStreamableHTTPHandler(func(request *http.Request) *mcp.Server {
		return server
	}, nil)
	return authenticate(token, streamable)
}

func jsonResult(value any) (*mcp.CallToolResult, any, error) {
	data, err := json.Marshal(value)
	if err != nil {
		return nil, nil, err
	}
	return &mcp.CallToolResult{Content: []mcp.Content{&mcp.TextContent{Text: string(data)}}}, value, nil
}

func countBoundRoutes(routes []model.Route) int {
	count := 0
	for _, route := range routes {
		if route.BindingID != "" {
			count++
		}
	}
	return count
}

func authenticate(token string, next http.Handler) http.Handler {
	return http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		response.Header().Set("Cache-Control", "no-store")
		response.Header().Set("X-Content-Type-Options", "nosniff")
		if !allowedOrigin(request.Header.Get("Origin")) {
			http.Error(response, "origin rejected", http.StatusForbidden)
			return
		}
		provided := strings.TrimSpace(strings.TrimPrefix(request.Header.Get("Authorization"), "Bearer "))
		if len(provided) != len(token) || subtle.ConstantTimeCompare([]byte(provided), []byte(token)) != 1 {
			response.Header().Set("WWW-Authenticate", "Bearer")
			http.Error(response, "unauthorized", http.StatusUnauthorized)
			return
		}
		request.Body = http.MaxBytesReader(response, request.Body, 1<<20)
		next.ServeHTTP(response, request)
	})
}

func allowedOrigin(origin string) bool {
	if origin == "" {
		return true
	}
	switch strings.ToLower(strings.TrimRight(origin, "/")) {
	case "http://127.0.0.1:8765", "http://localhost:8765", "http://127.0.0.1:8766", "http://localhost:8766":
		return true
	default:
		return false
	}
}

func ErrorText(err error) string {
	if err == nil {
		return ""
	}
	return fmt.Sprintf("%v", err)
}
