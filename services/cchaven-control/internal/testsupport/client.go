package testsupport

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/cookiejar"
	"strings"
	"testing"
)

// Client 是带 cookie jar 的测试 HTTP 客户端，模拟一个浏览器会话。
type Client struct {
	t       *testing.T
	base    string
	http    *http.Client
	headers map[string]string
}

// NewClient 建立一个新的浏览器会话（独立 cookie jar）。
func (e *Env) NewClient() *Client {
	e.T.Helper()

	jar, err := cookiejar.New(nil)
	if err != nil {
		e.T.Fatalf("创建 cookie jar 失败: %v", err)
	}

	return &Client{
		t:    e.T,
		base: e.Server.URL,
		http: &http.Client{
			Jar: jar,
			// 不自动跟随重定向，便于断言 OAuth 回调地址。
			CheckRedirect: func(*http.Request, []*http.Request) error {
				return http.ErrUseLastResponse
			},
		},
		// 服务端对 cookie 鉴权的写操作要求同源，测试固定带上可信 Origin。
		headers: map[string]string{"Origin": "https://cchaven.test"},
	}
}

// WithHeader 返回附加了指定请求头的客户端副本（共用同一个 cookie jar）。
func (c *Client) WithHeader(key, value string) *Client {
	clone := *c
	clone.headers = map[string]string{}
	for k, v := range c.headers {
		clone.headers[k] = v
	}
	clone.headers[key] = value
	return &clone
}

// WithBearer 返回使用 Bearer 令牌鉴权的客户端副本，模拟桌面 APP。
func (c *Client) WithBearer(token string) *Client {
	return c.WithHeader("Authorization", "Bearer "+token)
}

// Response 是解析后的响应。
type Response struct {
	t      *testing.T
	Status int
	Raw    []byte
	body   map[string]any
}

// Get 发起 GET 请求。
func (c *Client) Get(path string) *Response { return c.do(http.MethodGet, path, nil) }

// Post 发起 POST 请求，body 为 nil 时不带请求体。
func (c *Client) Post(path string, body any) *Response { return c.do(http.MethodPost, path, body) }

// Patch 发起 PATCH 请求。
func (c *Client) Patch(path string, body any) *Response { return c.do(http.MethodPatch, path, body) }

// Put 发起 PUT 请求。
func (c *Client) Put(path string, body any) *Response { return c.do(http.MethodPut, path, body) }

// Delete 发起 DELETE 请求。
func (c *Client) Delete(path string) *Response { return c.do(http.MethodDelete, path, nil) }

func (c *Client) do(method, path string, body any) *Response {
	c.t.Helper()

	var reader io.Reader
	if body != nil {
		encoded, err := json.Marshal(body)
		if err != nil {
			c.t.Fatalf("序列化请求体失败: %v", err)
		}
		reader = bytes.NewReader(encoded)
	}

	req, err := http.NewRequestWithContext(context.Background(), method, c.base+path, reader)
	if err != nil {
		c.t.Fatalf("构造请求失败: %v", err)
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	for k, v := range c.headers {
		req.Header.Set(k, v)
	}

	resp, err := c.http.Do(req)
	if err != nil {
		c.t.Fatalf("请求 %s %s 失败: %v", method, path, err)
	}
	defer resp.Body.Close()

	raw, err := io.ReadAll(resp.Body)
	if err != nil {
		c.t.Fatalf("读取响应失败: %v", err)
	}

	out := &Response{t: c.t, Status: resp.StatusCode, Raw: raw}
	if len(raw) > 0 && strings.HasPrefix(resp.Header.Get("Content-Type"), "application/json") {
		if err := json.Unmarshal(raw, &out.body); err != nil {
			c.t.Fatalf("解析响应 JSON 失败: %v (原文 %s)", err, raw)
		}
	}
	return out
}

// PostRaw 发送原始字节，用于支付回调这类需要精确控制报文的场景。
func (c *Client) PostRaw(path string, payload []byte, headers map[string]string) *Response {
	c.t.Helper()

	req, err := http.NewRequestWithContext(
		context.Background(), http.MethodPost, c.base+path, bytes.NewReader(payload))
	if err != nil {
		c.t.Fatalf("构造请求失败: %v", err)
	}
	req.Header.Set("Content-Type", "application/json")
	for k, v := range c.headers {
		req.Header.Set(k, v)
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}

	resp, err := c.http.Do(req)
	if err != nil {
		c.t.Fatalf("请求失败: %v", err)
	}
	defer resp.Body.Close()

	raw, _ := io.ReadAll(resp.Body)
	out := &Response{t: c.t, Status: resp.StatusCode, Raw: raw}
	if len(raw) > 0 && strings.HasPrefix(resp.Header.Get("Content-Type"), "application/json") {
		_ = json.Unmarshal(raw, &out.body)
	}
	return out
}

// ExpectStatus 断言 HTTP 状态码。
func (r *Response) ExpectStatus(want int) *Response {
	r.t.Helper()
	if r.Status != want {
		r.t.Fatalf("状态码不符: got %d want %d，响应 %s", r.Status, want, r.Raw)
	}
	return r
}

// Data 返回成功响应的 data 字段。
func (r *Response) Data() map[string]any {
	r.t.Helper()
	data, ok := r.body["data"].(map[string]any)
	if !ok {
		r.t.Fatalf("响应缺少 data 对象: %s", r.Raw)
	}
	return data
}

// ErrorCode 返回失败响应的错误码。
func (r *Response) ErrorCode() string {
	r.t.Helper()
	errObj, ok := r.body["error"].(map[string]any)
	if !ok {
		r.t.Fatalf("响应缺少 error 对象: %s", r.Raw)
	}
	code, _ := errObj["code"].(string)
	return code
}

// ErrorMessage 返回失败响应中下发给用户的文案。
func (r *Response) ErrorMessage() string {
	r.t.Helper()
	errObj, ok := r.body["error"].(map[string]any)
	if !ok {
		r.t.Fatalf("响应缺少 error 对象: %s", r.Raw)
	}
	message, _ := errObj["message"].(string)
	return message
}

// String 读取 data 下的字符串字段。
func (r *Response) String(key string) string {
	r.t.Helper()
	value, _ := r.Data()[key].(string)
	return value
}

// Number 读取 data 下的数值字段。
func (r *Response) Number(key string) float64 {
	r.t.Helper()
	value, _ := r.Data()[key].(float64)
	return value
}

// Object 读取 data 下的对象字段。
func (r *Response) Object(key string) map[string]any {
	r.t.Helper()
	value, ok := r.Data()[key].(map[string]any)
	if !ok {
		r.t.Fatalf("data.%s 不是对象: %s", key, r.Raw)
	}
	return value
}

// Array 读取 data 下的数组字段。
func (r *Response) Array(key string) []any {
	r.t.Helper()
	value, _ := r.Data()[key].([]any)
	return value
}
