package registration

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestFeishuDeviceRegistrationFlow(t *testing.T) {
	polls := 0
	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		if err := request.ParseForm(); err != nil {
			t.Fatal(err)
		}
		switch request.Form.Get("action") {
		case "begin":
			if request.Form.Get("archetype") != "PersonalAgent" {
				t.Fatalf("archetype = %q", request.Form.Get("archetype"))
			}
			_ = json.NewEncoder(response).Encode(map[string]any{
				"device_code": "device-1", "verification_uri_complete": "https://open.feishu.cn/page/launcher?user_code=ABCD", "expires_in": 600, "interval": 1,
			})
		case "poll":
			polls++
			if polls == 1 {
				response.WriteHeader(http.StatusBadRequest)
				_ = json.NewEncoder(response).Encode(map[string]any{"error": "authorization_pending"})
				return
			}
			_ = json.NewEncoder(response).Encode(map[string]any{"client_id": "cli_test", "client_secret": "secret-value-123456"})
		default:
			http.Error(response, "unknown action", http.StatusBadRequest)
		}
	}))
	defer server.Close()

	started, err := BeginFeishu(context.Background(), server.URL)
	if err != nil || started.DeviceCode != "device-1" || started.IntervalSeconds != 1 {
		t.Fatalf("started = %#v, err = %v", started, err)
	}
	first, err := PollFeishu(context.Background(), server.URL, started.DeviceCode)
	if err != nil || first.Status != "pending" {
		t.Fatalf("first poll = %#v, err = %v", first, err)
	}
	second, err := PollFeishu(context.Background(), server.URL, started.DeviceCode)
	if err != nil || second.Status != "completed" || second.ClientID != "cli_test" || second.ClientSecret == "" {
		t.Fatalf("second poll = %#v, err = %v", second, err)
	}
}
