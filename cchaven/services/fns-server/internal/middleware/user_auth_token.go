package middleware

import (
	"context"
	"net/http"
	"net/url"
	"strings"

	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	"github.com/haierkeys/fast-note-sync-service/pkg/app"
	"github.com/haierkeys/fast-note-sync-service/pkg/code"
	"github.com/haierkeys/fast-note-sync-service/pkg/util"

	"github.com/gin-gonic/gin"
)

func UserAuthTokenWithConfig(secretKey string, tokenService service.TokenService) gin.HandlerFunc {
	return func(c *gin.Context) {
		response := app.NewResponse(c)
		user, scope, vaults, dbToken, appErr := AuthenticateUserToken(c, secretKey, tokenService)
		if appErr != nil {
			response.ToResponse(appErr)
			c.Abort()
			return
		}
		c.Set("user_token", user)
		c.Set("scope", scope)
		c.Set("vaults", vaults)
		// Inject server-side token attributes for downstream middleware (e.g. RequireWebGUI)
		// 注入服务端 Token 属性，供下游中间件（如 RequireWebGUI）进行联合校验，防止请求头伪造绕过
		c.Set("token_issue_type", dbToken.IssueType)
		c.Set("token_client_type", dbToken.ClientType)
		c.Next()
	}
}

func ExtractUserAuthToken(c *gin.Context) string {
	authHeader := c.GetHeader("Authorization")
	if authHeader != "" && strings.HasPrefix(authHeader, "Bearer ") {
		return strings.TrimPrefix(authHeader, "Bearer ")
	}

	if authHeader := c.GetHeader("Token"); authHeader != "" {
		return authHeader
	}
	if authHeader := c.GetHeader("token"); authHeader != "" {
		return authHeader
	}

	return c.Query("token")
}

type AuthenticatedUserToken struct {
	User          *app.UserEntity
	Token         *domain.AuthToken
	Scope         string
	Vaults        string
	ClientType    string
	ClientName    string
	ClientVersion string
}

func AuthenticateUserToken(c *gin.Context, secretKey string, tokenService service.TokenService) (*app.UserEntity, string, string, *domain.AuthToken, *code.Code) {
	token := ExtractUserAuthToken(c)
	if token == "" {
		return nil, "", "", nil, code.ErrorNotUserAuthToken
	}
	path := c.Request.URL.Path
	method := c.Request.Method
	var function string
	var resource string
	if strings.HasPrefix(path, "/api/note") || strings.HasPrefix(path, "/api/folder") {
		resource = "note"
	} else if strings.HasPrefix(path, "/api/file") || strings.HasPrefix(path, "/api/storage") {
		resource = "file"
	} else if strings.HasPrefix(path, "/api/setting") || strings.HasPrefix(path, "/api/admin/config") {
		resource = "config"
	}
	if resource != "" {
		if method == http.MethodGet || method == http.MethodHead || method == http.MethodOptions {
			function = resource + "_r"
		} else {
			function = resource + "_w"
		}
	}
	protocol := "rest"
	if strings.HasPrefix(path, "/api/mcp") {
		protocol = "mcp"
	}

	identity, appErr := authenticateUserTokenForProtocol(c, secretKey, tokenService, token, protocol, "", function, true, false)
	if appErr != nil {
		return nil, "", "", nil, appErr
	}
	dbToken := identity.Token
	user := identity.User

	if dbToken.Vaults != "" {
		targetVault := app.RequestParam(c, "vault")
		if targetVault != "" && !util.VerifyVaultAccess(dbToken.Vaults, targetVault) {
			return nil, "", "", nil, code.ErrorAuthTokenScopeRestricted.WithDetails("Vault access restricted: " + targetVault)
		}
	}

	recordAuthTokenAccess(c, tokenService, identity, protocol)

	return user, dbToken.Scope, dbToken.Vaults, dbToken, nil
}

func AuthenticateBearerUserTokenForProtocol(
	c *gin.Context,
	secretKey string,
	tokenService service.TokenService,
	protocol string,
	requiredClient string,
	requiredFunction string,
) (*AuthenticatedUserToken, *code.Code) {
	token, ok := extractBearerToken(c)
	if !ok {
		return nil, code.ErrorNotUserAuthToken
	}
	identity, appErr := authenticateUserTokenForProtocol(
		c,
		secretKey,
		tokenService,
		token,
		protocol,
		requiredClient,
		requiredFunction,
		false,
		true,
	)
	if appErr != nil {
		return nil, appErr
	}
	return identity, nil
}

func extractBearerToken(c *gin.Context) (string, bool) {
	if c == nil || c.Request == nil {
		return "", false
	}
	values := c.Request.Header.Values("Authorization")
	if len(values) != 1 {
		return "", false
	}
	value := values[0]
	const scheme = "Bearer"
	if len(value) <= len(scheme) || !strings.EqualFold(value[:len(scheme)], scheme) || value[len(scheme)] != ' ' {
		return "", false
	}
	token := strings.TrimLeft(value[len(scheme):], " ")
	if token == "" || strings.ContainsAny(token, " \t\r\n") {
		return "", false
	}
	return token, true
}

func authenticateUserTokenForProtocol(
	c *gin.Context,
	secretKey string,
	tokenService service.TokenService,
	token string,
	protocol string,
	requiredClient string,
	function string,
	allowLegacyClientSources bool,
	requireExactPermission bool,
) (*AuthenticatedUserToken, *code.Code) {
	user, err := app.ParseTokenWithKey(token, secretKey)
	if err != nil {
		if appErr, ok := err.(*code.Code); ok {
			return nil, appErr
		}
		return nil, code.ErrorInvalidUserAuthToken
	}
	if tokenService == nil {
		return nil, code.ErrorInvalidUserAuthToken
	}
	dbToken, err := tokenService.GetActiveToken(c.Request.Context(), user.UID, user.TokenID)
	if err != nil || dbToken == nil {
		if appErr, ok := err.(*code.Code); ok {
			return nil, appErr
		}
		return nil, code.ErrorInvalidUserAuthToken
	}
	if dbToken.TokenString != "" && user.Nonce != dbToken.TokenString {
		return nil, code.ErrorInvalidUserAuthToken.WithDetails("Token has been rotated")
	}

	reqClientType := c.GetHeader("x-client")
	if allowLegacyClientSources && reqClientType == "" {
		reqClientType = c.Query("client")
	}
	if allowLegacyClientSources && reqClientType == "" && dbToken.IssueType == 1 && isHeaderlessLoginTokenResourceRead(c) {
		reqClientType = dbToken.ClientType
	}
	if dbToken.IssueType == 1 && !app.MatchWildcard(dbToken.ClientType, reqClientType) {
		return nil, code.ErrorAuthTokenClientRestricted.WithDetails("Client mismatch")
	}
	if dbToken.UserAgent != "" && !app.MatchWildcard(dbToken.UserAgent, c.GetHeader("User-Agent")) {
		return nil, code.ErrorAuthTokenUARestricted.WithDetails("User-Agent mismatch")
	}
	if dbToken.BoundIP != "" && !app.MatchWildcard(dbToken.BoundIP, c.ClientIP()) {
		return nil, code.ErrorAuthTokenIPRestricted.WithDetails("IP mismatch")
	}

	authorized := true
	if requireExactPermission {
		authorized = reqClientType == requiredClient &&
			app.VerifyExactPermissions(dbToken.Scope, protocol, requiredClient, function)
	} else if c.Request.URL.Path != "/api/health" {
		authorized = app.VerifyPermissions(dbToken.Scope, protocol, reqClientType, function)
	}
	if !authorized {
		resPath := c.Query("path")
		if resPath == "" {
			resPath = c.Query("name")
		}
		if resPath == "" {
			resPath = c.Query("file")
		}
		if resPath == "" {
			resPath = c.Request.URL.Path
		}
		return nil, code.ErrorAuthTokenScopeRestricted.WithDetails("Permission denied: " + resPath)
	}

	clientName := c.GetHeader("x-client-name")
	if clientName != "" {
		if decoded, decodeErr := url.QueryUnescape(clientName); decodeErr == nil {
			clientName = decoded
		}
	}
	return &AuthenticatedUserToken{
		User:          user,
		Token:         dbToken,
		Scope:         dbToken.Scope,
		Vaults:        dbToken.Vaults,
		ClientType:    reqClientType,
		ClientName:    clientName,
		ClientVersion: c.GetHeader("x-client-version"),
	}, nil
}

func recordAuthTokenAccess(c *gin.Context, tokenService service.TokenService, identity *AuthenticatedUserToken, protocol string) {
	if c == nil || tokenService == nil || identity == nil || identity.Token == nil {
		return
	}
	log := &domain.AuthTokenLog{
		TokenID:       identity.Token.ID,
		UID:           identity.Token.UID,
		Protocol:      protocol,
		Client:        identity.ClientType,
		ClientName:    identity.ClientName,
		ClientVersion: identity.ClientVersion,
		IP:            c.ClientIP(),
		UA:            c.GetHeader("User-Agent"),
		StatusCode:    int64(c.Writer.Status()),
	}
	go func() {
		_ = tokenService.RecordAccessLog(context.Background(), log)
	}()
}

func isHeaderlessLoginTokenResourceRead(c *gin.Context) bool {
	return c.Request.URL.Path == "/api/file" &&
		(c.Request.Method == http.MethodGet || c.Request.Method == http.MethodHead)
}

// UserAuthToken user Token authentication middleware (no secret key, always fails)
// UserAuthToken 用户 Token 认证中间件（无密钥，始终失败）
// Deprecated: Use UserAuthTokenWithConfig instead
// Deprecated: 推荐使用 UserAuthTokenWithConfig
func UserAuthToken() gin.HandlerFunc {
	// Without token service this cannot work properly in 3D RBAC
	return UserAuthTokenWithConfig("", nil)
}
