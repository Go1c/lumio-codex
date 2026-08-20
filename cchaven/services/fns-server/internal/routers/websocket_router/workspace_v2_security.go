package websocket_router

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
)

type WorkspaceV2AccessError struct {
	Code   string
	Field  string
	Reason string
}

func (e *WorkspaceV2AccessError) Error() string {
	if e == nil {
		return ""
	}
	if e.Field == "" {
		return fmt.Sprintf("%s: %s", e.Code, e.Reason)
	}
	return fmt.Sprintf("%s: %s", e.Field, e.Reason)
}

type WorkspaceV2AccessPolicy struct {
	maxWorkspacesPerUser int
	roots                map[int64]map[dto.WorkspaceUUID]string
}

func NewWorkspaceV2AccessPolicy(cfg config.WorkspaceConfig) *WorkspaceV2AccessPolicy {
	policy := &WorkspaceV2AccessPolicy{
		maxWorkspacesPerUser: cfg.MaxWorkspacesPerUser,
		roots:                make(map[int64]map[dto.WorkspaceUUID]string),
	}
	for _, root := range cfg.Roots {
		if policy.roots[root.UID] == nil {
			policy.roots[root.UID] = make(map[dto.WorkspaceUUID]string)
		}
		policy.roots[root.UID][dto.WorkspaceUUID(root.WorkspaceID)] = root.Root
	}
	return policy
}

func (p *WorkspaceV2AccessPolicy) Authorize(uid int64, workspaceID dto.WorkspaceUUID) error {
	if p == nil {
		return workspaceV2Forbidden("workspaceId", "not_allowed")
	}
	if _, err := dto.ParseWorkspaceUUID("workspaceId", string(workspaceID)); err != nil {
		return workspaceV2Forbidden("workspaceId", "not_allowed")
	}
	if workspaces := p.roots[uid]; workspaces != nil {
		if _, ok := workspaces[workspaceID]; ok {
			return nil
		}
	}
	return workspaceV2Forbidden("workspaceId", "not_allowed")
}

func (p *WorkspaceV2AccessPolicy) CheckPath(uid int64, workspaceID dto.WorkspaceUUID, path dto.WorkspacePath) error {
	if err := p.Authorize(uid, workspaceID); err != nil {
		return err
	}
	canonicalPath, err := dto.ParseWorkspacePath(string(path))
	if err != nil {
		return workspaceV2InvalidPath(workspaceV2PathReason(err))
	}
	root := p.roots[uid][workspaceID]
	canonicalRoot, err := filepath.EvalSymlinks(root)
	if err != nil || canonicalRoot != root {
		return workspaceV2InvalidPath("root_symlink")
	}

	current := root
	for _, segment := range strings.Split(string(canonicalPath), "/") {
		candidate := filepath.Join(current, filepath.FromSlash(segment))
		info, statErr := os.Lstat(candidate)
		if errors.Is(statErr, os.ErrNotExist) {
			return nil
		}
		if statErr != nil {
			return workspaceV2InvalidPath("filesystem_error")
		}
		if info.Mode()&os.ModeSymlink != 0 {
			resolved, resolveErr := filepath.EvalSymlinks(candidate)
			if resolveErr != nil {
				return workspaceV2InvalidPath("symlink_unresolvable")
			}
			if !workspaceV2PathWithinRoot(canonicalRoot, resolved) {
				return workspaceV2InvalidPath("outside_root")
			}
			current = resolved
			continue
		}
		current = candidate
	}
	return nil
}

func workspaceV2PathWithinRoot(root, candidate string) bool {
	relative, err := filepath.Rel(root, candidate)
	return err == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator))
}

func workspaceV2Forbidden(field, reason string) *WorkspaceV2AccessError {
	return &WorkspaceV2AccessError{Code: "forbidden", Field: field, Reason: reason}
}

func workspaceV2InvalidPath(reason string) *WorkspaceV2AccessError {
	return &WorkspaceV2AccessError{Code: "invalid_path", Field: "data.path", Reason: reason}
}

func workspaceV2PathReason(err error) string {
	var validationErr *dto.WorkspaceValidationError
	if errors.As(err, &validationErr) {
		return validationErr.Reason
	}
	return "invalid_path"
}
