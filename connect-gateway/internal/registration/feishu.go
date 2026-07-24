package registration

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

const DefaultFeishuBaseURL = "https://accounts.feishu.cn"

type FeishuBeginResult struct {
	DeviceCode              string `json:"deviceCode"`
	VerificationURIComplete string `json:"verificationUrl"`
	ExpiresAt               int64  `json:"expiresAt"`
	IntervalSeconds         int    `json:"intervalSeconds"`
}

type FeishuPollResult struct {
	Status       string `json:"status"`
	ClientID     string `json:"appId,omitempty"`
	ClientSecret string `json:"appSecret,omitempty"`
	Error        string `json:"error,omitempty"`
}

type registrationResponse struct {
	DeviceCode              string `json:"device_code"`
	VerificationURIComplete string `json:"verification_uri_complete"`
	ExpiresIn               int64  `json:"expires_in"`
	Interval                int    `json:"interval"`
	ClientID                string `json:"client_id"`
	ClientSecret            string `json:"client_secret"`
	Error                   string `json:"error"`
	ErrorDescription        string `json:"error_description"`
}

func BeginFeishu(ctx context.Context, baseURL string) (FeishuBeginResult, error) {
	response, err := request(ctx, baseURL, url.Values{
		"action":            {"begin"},
		"archetype":         {"PersonalAgent"},
		"auth_method":       {"client_secret"},
		"request_user_info": {"open_id"},
	})
	if err != nil {
		return FeishuBeginResult{}, err
	}
	if response.DeviceCode == "" || response.VerificationURIComplete == "" {
		return FeishuBeginResult{}, errors.New("Feishu registration did not return a verification link")
	}
	if response.ExpiresIn <= 0 {
		response.ExpiresIn = 600
	}
	if response.Interval <= 0 {
		response.Interval = 5
	}
	verificationURL, err := url.Parse(response.VerificationURIComplete)
	if err != nil || verificationURL.Scheme != "https" {
		return FeishuBeginResult{}, errors.New("Feishu registration returned an invalid verification link")
	}
	query := verificationURL.Query()
	query.Set("from", "sdk")
	query.Set("source", "node-sdk/csa-connect")
	query.Set("tp", "sdk")
	query.Set("name", "CSA Research Assistant")
	query.Set("desc", "Claude Science 本地科研会话连接器")
	verificationURL.RawQuery = query.Encode()
	return FeishuBeginResult{
		DeviceCode:              response.DeviceCode,
		VerificationURIComplete: verificationURL.String(),
		ExpiresAt:               time.Now().UTC().Add(time.Duration(response.ExpiresIn) * time.Second).UnixMilli(),
		IntervalSeconds:         response.Interval,
	}, nil
}

func PollFeishu(ctx context.Context, baseURL, deviceCode string) (FeishuPollResult, error) {
	deviceCode = strings.TrimSpace(deviceCode)
	if deviceCode == "" || len(deviceCode) > 2048 || strings.ContainsAny(deviceCode, "\r\n\x00") {
		return FeishuPollResult{}, errors.New("Feishu registration device code is invalid")
	}
	response, err := request(ctx, baseURL, url.Values{
		"action":      {"poll"},
		"device_code": {deviceCode},
	})
	if err != nil {
		return FeishuPollResult{}, err
	}
	if response.ClientID != "" && response.ClientSecret != "" {
		return FeishuPollResult{Status: "completed", ClientID: response.ClientID, ClientSecret: response.ClientSecret}, nil
	}
	switch response.Error {
	case "authorization_pending", "slow_down", "":
		return FeishuPollResult{Status: "pending"}, nil
	case "access_denied", "expired_token":
		return FeishuPollResult{Status: "failed", Error: response.Error}, nil
	default:
		return FeishuPollResult{Status: "failed", Error: response.Error}, nil
	}
}

func request(ctx context.Context, baseURL string, values url.Values) (registrationResponse, error) {
	baseURL = strings.TrimRight(strings.TrimSpace(baseURL), "/")
	if baseURL == "" {
		baseURL = DefaultFeishuBaseURL
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, baseURL+"/oauth/v1/app/registration", strings.NewReader(values.Encode()))
	if err != nil {
		return registrationResponse{}, errors.New("build Feishu registration request failed")
	}
	request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	client := &http.Client{Timeout: 15 * time.Second}
	response, err := client.Do(request)
	if err != nil {
		return registrationResponse{}, errors.New("Feishu registration service is unavailable")
	}
	defer response.Body.Close()
	body, err := io.ReadAll(io.LimitReader(response.Body, 1<<20))
	if err != nil {
		return registrationResponse{}, errors.New("read Feishu registration response failed")
	}
	var decoded registrationResponse
	if err := json.Unmarshal(body, &decoded); err != nil {
		return registrationResponse{}, errors.New("Feishu registration returned invalid JSON")
	}
	if response.StatusCode >= 500 {
		return registrationResponse{}, fmt.Errorf("Feishu registration returned HTTP %d", response.StatusCode)
	}
	if response.StatusCode >= 400 && decoded.Error == "" {
		return registrationResponse{}, fmt.Errorf("Feishu registration returned HTTP %d", response.StatusCode)
	}
	return decoded, nil
}
