// Package sub2api 是 Sub2API（Lumio 账号中心）的客户端。
//
// 本服务不再自持终端用户账号：邮箱、口令、账号状态与余额全部由 Sub2API 保管，
// 控制面只拿着调用方出示的 access token 去 Sub2API 换回身份，再把它映射到
// 本地的 CC 业务数据（订阅 / 邀请 / 设备）。写余额走 Debit。
//
// 两条硬约束：
//   - 校验结果带短 TTL 缓存，避免每个请求都打一次外部 API；
//   - 上游不可用时返回 ErrUnavailable / ErrDebitUnavailable 让调用方渲染 503，**绝不静默放行**。
package sub2api

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	// DefaultBaseURL 是 Sub2API 的线上地址。
	DefaultBaseURL = "https://api.lumio.games"
	// MePath 是校验令牌并取回身份的端点。
	MePath = "/api/v1/auth/me"
	// DefaultCacheTTL 是身份缓存的默认有效期。
	//
	// 取值是「少打外部 API」与「Sub2API 停用账号后多久生效」的折衷：
	// 一分钟内的过期窗口可以接受，写操作出问题时还有各业务自身的状态校验兜底。
	DefaultCacheTTL = time.Minute
	// DefaultTimeout 是单次上游调用的超时。
	DefaultTimeout = 5 * time.Second

	// maxCacheEntries 是缓存条目上限，防止令牌轮换把内存撑爆。
	maxCacheEntries = 10000
	// maxBodyBytes 限制读取的响应体大小。
	maxBodyBytes = 1 << 20
)

var (
	// ErrInvalidToken 表示令牌无效或已过期，调用方应返回 401。
	ErrInvalidToken = errors.New("sub2api: 令牌无效")
	// ErrUnavailable 表示无法向 Sub2API 求证，调用方应返回 503 而不是放行。
	ErrUnavailable = errors.New("sub2api: 身份服务不可用")
)

// Identity 是 Sub2API 侧的用户身份快照。
type Identity struct {
	ID      string  `json:"id"`
	Email   string  `json:"email"`
	Balance float64 `json:"balance"`
	Status  string  `json:"status"`
}

// Active 报告账号在 Sub2API 侧是否可用。
// 上游不下发 status 时按可用处理——停用账号本就拿不到有效令牌。
func (i Identity) Active() bool {
	switch strings.ToLower(strings.TrimSpace(i.Status)) {
	case "", "active", "ok", "enabled", "normal":
		return true
	default:
		return false
	}
}

// Options 是构造客户端的参数，零值即为线上默认配置。
type Options struct {
	BaseURL    string
	CacheTTL   time.Duration
	HTTPClient *http.Client
	// Now 允许测试注入可控时钟；生产为 time.Now。
	Now func() time.Time
}

// Client 校验 Sub2API 令牌并缓存结果。可被多个请求并发使用。
type Client struct {
	baseURL string
	http    *http.Client
	ttl     time.Duration
	now     func() time.Time

	mu    sync.Mutex
	cache map[string]cacheEntry
}

type cacheEntry struct {
	identity  Identity
	expiresAt time.Time
}

// New 构造客户端。
func New(opts Options) *Client {
	base := strings.TrimRight(strings.TrimSpace(opts.BaseURL), "/")
	if base == "" {
		base = DefaultBaseURL
	}
	ttl := opts.CacheTTL
	if ttl <= 0 {
		ttl = DefaultCacheTTL
	}
	httpClient := opts.HTTPClient
	if httpClient == nil {
		httpClient = &http.Client{Timeout: DefaultTimeout}
	}
	now := opts.Now
	if now == nil {
		now = time.Now
	}

	return &Client{
		baseURL: base,
		http:    httpClient,
		ttl:     ttl,
		now:     now,
		cache:   map[string]cacheEntry{},
	}
}

// BaseURL 返回归一化后的上游地址。
func (c *Client) BaseURL() string { return c.baseURL }

// Verify 校验令牌并返回身份。
func (c *Client) Verify(ctx context.Context, token string) (Identity, error) {
	token = strings.TrimSpace(token)
	if token == "" {
		return Identity{}, ErrInvalidToken
	}

	key := cacheKey(token)
	if identity, ok := c.lookup(key); ok {
		return identity, nil
	}

	identity, err := c.fetch(ctx, token)
	if err != nil {
		return Identity{}, err
	}

	c.store(key, identity)
	return identity, nil
}

// VerifyFresh 强制回源校验令牌并写回缓存，不得只读旧条目。
// 扣费前要用它拿到当前余额，TTL 内的 Verify 快照不够。
func (c *Client) VerifyFresh(ctx context.Context, token string) (Identity, error) {
	token = strings.TrimSpace(token)
	if token == "" {
		return Identity{}, ErrInvalidToken
	}

	identity, err := c.fetch(ctx, token)
	if err != nil {
		return Identity{}, err
	}
	c.store(cacheKey(token), identity)
	return identity, nil
}

func (c *Client) fetch(ctx context.Context, token string) (Identity, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.baseURL+MePath, nil)
	if err != nil {
		return Identity{}, fmt.Errorf("%w: 构造请求失败: %v", ErrUnavailable, err)
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/json")

	resp, err := c.http.Do(req)
	if err != nil {
		return Identity{}, fmt.Errorf("%w: %v", ErrUnavailable, err)
	}
	defer resp.Body.Close()

	switch {
	case resp.StatusCode == http.StatusUnauthorized, resp.StatusCode == http.StatusForbidden:
		return Identity{}, ErrInvalidToken
	case resp.StatusCode != http.StatusOK:
		return Identity{}, fmt.Errorf("%w: 上游返回 HTTP %d", ErrUnavailable, resp.StatusCode)
	}

	body, err := io.ReadAll(io.LimitReader(resp.Body, maxBodyBytes))
	if err != nil {
		return Identity{}, fmt.Errorf("%w: 读取响应失败: %v", ErrUnavailable, err)
	}

	identity, err := parseIdentity(body)
	if err != nil {
		return Identity{}, err
	}
	return identity, nil
}

func (c *Client) lookup(key string) (Identity, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()

	entry, ok := c.cache[key]
	if !ok {
		return Identity{}, false
	}
	if !entry.expiresAt.After(c.now()) {
		delete(c.cache, key)
		return Identity{}, false
	}
	return entry.identity, true
}

func (c *Client) store(key string, identity Identity) {
	now := c.now()

	c.mu.Lock()
	defer c.mu.Unlock()

	if len(c.cache) >= maxCacheEntries {
		for k, entry := range c.cache {
			if !entry.expiresAt.After(now) {
				delete(c.cache, k)
			}
		}
		// 过期条目清不出空间时整桶丢弃：缓存只是省调用，重建的代价可控。
		if len(c.cache) >= maxCacheEntries {
			c.cache = map[string]cacheEntry{}
		}
	}
	c.cache[key] = cacheEntry{identity: identity, expiresAt: now.Add(c.ttl)}
}

// cacheKey 以摘要作键，令牌明文不进长生命周期的数据结构。
func cacheKey(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:])
}

// meBody 兼容 Sub2API 的两种信封：`{"data":{…}}` 与裸对象。
type meBody struct {
	ID      flexString `json:"id"`
	Email   string     `json:"email"`
	Balance flexFloat  `json:"balance"`
	Status  string     `json:"status"`
}

func parseIdentity(raw []byte) (Identity, error) {
	var envelope struct {
		Data json.RawMessage `json:"data"`
	}
	payload := raw
	if err := json.Unmarshal(raw, &envelope); err == nil && len(envelope.Data) > 0 {
		payload = envelope.Data
	}

	var body meBody
	if err := json.Unmarshal(payload, &body); err != nil {
		return Identity{}, fmt.Errorf("%w: 响应无法解析: %v", ErrUnavailable, err)
	}
	if strings.TrimSpace(string(body.ID)) == "" {
		return Identity{}, fmt.Errorf("%w: 响应缺少用户 id", ErrUnavailable)
	}

	return Identity{
		ID:      string(body.ID),
		Email:   strings.TrimSpace(body.Email),
		Balance: float64(body.Balance),
		Status:  body.Status,
	}, nil
}

// flexString 接受字符串或数字形式的标识符。
type flexString string

func (f *flexString) UnmarshalJSON(data []byte) error {
	trimmed := strings.TrimSpace(string(data))
	if trimmed == "null" {
		*f = ""
		return nil
	}
	if strings.HasPrefix(trimmed, `"`) {
		var s string
		if err := json.Unmarshal(data, &s); err != nil {
			return err
		}
		*f = flexString(strings.TrimSpace(s))
		return nil
	}
	*f = flexString(trimmed)
	return nil
}

// flexFloat 接受数字或字符串形式的金额。
type flexFloat float64

func (f *flexFloat) UnmarshalJSON(data []byte) error {
	trimmed := strings.TrimSpace(string(data))
	if trimmed == "null" || trimmed == `""` {
		*f = 0
		return nil
	}
	if strings.HasPrefix(trimmed, `"`) {
		var s string
		if err := json.Unmarshal(data, &s); err != nil {
			return err
		}
		value, err := strconv.ParseFloat(strings.TrimSpace(s), 64)
		if err != nil {
			return err
		}
		*f = flexFloat(value)
		return nil
	}

	var value float64
	if err := json.Unmarshal(data, &value); err != nil {
		return err
	}
	*f = flexFloat(value)
	return nil
}
