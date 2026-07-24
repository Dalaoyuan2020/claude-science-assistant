package mcpserver

import (
	"context"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"testing"

	"github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/gateway"
	"github.com/csa/claude-science-api-bridge/connect-gateway/internal/store"
)

type authTransport struct {
	token string
	base  http.RoundTripper
}

func (transport authTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	clone := request.Clone(request.Context())
	clone.Header.Set("Authorization", "Bearer "+transport.token)
	return transport.base.RoundTrip(clone)
}

func TestMCPAuthenticationAndStatusTool(t *testing.T) {
	dataStore, err := store.Open(filepath.Join(t.TempDir(), "connect.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer dataStore.Close()
	token := "0123456789abcdef0123456789abcdef"
	server := httptest.NewServer(NewHandler(gateway.New(dataStore), token))
	defer server.Close()

	response, err := http.Post(server.URL, "application/json", nil)
	if err != nil {
		t.Fatal(err)
	}
	response.Body.Close()
	if response.StatusCode != http.StatusUnauthorized {
		t.Fatalf("unauthorized status = %d", response.StatusCode)
	}

	client := mcp.NewClient(&mcp.Implementation{Name: "test", Version: "1"}, nil)
	httpClient := &http.Client{Transport: authTransport{token: token, base: http.DefaultTransport}}
	session, err := client.Connect(context.Background(), &mcp.StreamableClientTransport{
		Endpoint:             server.URL,
		HTTPClient:           httpClient,
		DisableStandaloneSSE: true,
	}, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer session.Close()
	tools, err := session.ListTools(context.Background(), nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(tools.Tools) != 5 {
		t.Fatalf("tool count = %d", len(tools.Tools))
	}
	foundProgress := false
	for _, tool := range tools.Tools {
		if tool.Name == "connect_send_progress" {
			foundProgress = true
		}
	}
	if !foundProgress {
		t.Fatal("connect_send_progress tool is missing")
	}
	result, err := session.CallTool(context.Background(), &mcp.CallToolParams{Name: "connect_get_status", Arguments: map[string]any{}})
	if err != nil || len(result.Content) != 1 {
		t.Fatalf("status result = %#v, err = %v", result, err)
	}
}
