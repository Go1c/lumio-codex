package app

import (
	"strings"
)

// VerifyPermissions verifies if the given scope matches the required protocol, client, and function.
// VerifyPermissions 验证给定的作用域是否匹配所需的协议、客户端和功能。
// scope: The permission string (e.g. "p:rest c:webgui f:note_r") // 权限范围
// p: The protocol of the current request (e.g. "rest", "ws", "mcp") // 当前请求的协议
// c: The client of the current request (e.g. "webgui", "obsidian", "mobile") // 当前请求的客户端
// f: The function being accessed (e.g. "note_r", "note_w"). If empty, it means the resource is not restricted by function level. // 访问的功能。如果为空，表示该资源不受功能级别限制。
func VerifyPermissions(scope string, p string, c string, f string) bool {
	scope = strings.TrimSpace(scope)
	if scope == "" {
		return true
	}

	parts := strings.Split(scope, " ")

	var scopeP, scopeC, scopeF string

	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part == "" {
			continue
		}
		if strings.HasPrefix(part, "p:") {
			scopeP = part[2:]
		} else if strings.HasPrefix(part, "c:") {
			scopeC = part[2:]
		} else if strings.HasPrefix(part, "f:") {
			scopeF = part[2:]
		}
	}

	// Match Protocol (supports comma-separated list, e.g. "rest,ws")
	matchP := true
	if scopeP != "" && scopeP != "*" {
		matchP = strings.EqualFold(scopeP, p)
		if !matchP {
			pList := strings.Split(scopeP, ",")
			for _, item := range pList {
				if strings.EqualFold(strings.TrimSpace(item), p) {
					matchP = true
					break
				}
			}
		}
	}

	// Match Client (Only if scope specifies a client)
	matchC := true
	if scopeC != "" && scopeC != "*" {
		matchC = MatchWildcard(scopeC, c)
	}

	matchF := true
	if f != "" && scopeF != "" && scopeF != "*" {
		fList := strings.Split(scopeF, ",")
		matchF = false
		for _, item := range fList {
			item = strings.TrimSpace(item)
			// Direct match
			if strings.EqualFold(item, f) {
				matchF = true
				break
			}
			// _rw matches both _r and _w
			if strings.HasSuffix(item, "_rw") {
				prefix := item[:len(item)-3]
				if strings.HasPrefix(f, prefix) && (strings.HasSuffix(f, "_r") || strings.HasSuffix(f, "_w")) {
					matchF = true
					break
				}
			}
			// _w also implies _r for the same resource (backward compatibility)
			if strings.HasSuffix(item, "_w") && strings.HasSuffix(f, "_r") {
				if item[:len(item)-2] == f[:len(f)-2] {
					matchF = true
					break
				}
			}
		}
	}

	return matchP && matchC && matchF
}

// VerifyExactPermissions requires one explicit protocol, client, and function
// dimension. Wildcards, lists, duplicates, missing dimensions, and unknown
// dimensions fail closed.
func VerifyExactPermissions(scope, protocol, client, function string) bool {
	if strings.TrimSpace(scope) == "" || strings.TrimSpace(protocol) == "" ||
		strings.TrimSpace(client) == "" || strings.TrimSpace(function) == "" {
		return false
	}

	required := map[string]string{
		"p": protocol,
		"c": client,
		"f": function,
	}
	seen := make(map[string]bool, len(required))
	parts := strings.Fields(scope)
	if len(parts) != len(required) {
		return false
	}
	for _, part := range parts {
		dimension, value, ok := strings.Cut(part, ":")
		expected, known := required[dimension]
		if !ok || !known || value == "" || seen[dimension] || !strings.EqualFold(value, expected) {
			return false
		}
		seen[dimension] = true
	}
	return len(seen) == len(required)
}

// MatchWildcard checks if a value matches a pattern with an optional wildcard suffix '*'
func MatchWildcard(pattern, value string) bool {
	pattern = strings.ToLower(strings.TrimSpace(pattern))
	value = strings.ToLower(strings.TrimSpace(value))

	if pattern == "" || pattern == "*" {
		return true
	}
	if strings.HasSuffix(pattern, "*") {
		return strings.HasPrefix(value, strings.TrimSuffix(pattern, "*"))
	}
	return pattern == value
}

// Is3DRBACScope checks if the scope string follows the 3D-RBAC format (p: c: f:)
func Is3DRBACScope(scope string) bool {
	return strings.Contains(scope, "p:") || strings.Contains(scope, "c:") || strings.Contains(scope, "f:")
}
