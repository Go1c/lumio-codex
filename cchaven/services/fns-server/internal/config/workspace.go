package config

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/pkg/util"
)

const (
	WorkspaceMaxPerUser           = 8
	maxWorkspacePaths             = 50_000
	maxWorkspaceBytes      uint64 = 5_368_709_120
	maxWorkspacePruneBatch        = 10_000
)

type WorkspaceRootConfig struct {
	UID         int64  `yaml:"uid"`
	WorkspaceID string `yaml:"workspace-id"`
	Root        string `yaml:"root"`
}

type WorkspaceConfig struct {
	BlobPath             string                `yaml:"blob-path" default:"storage/workspace-blobs"`
	MaxPaths             int                   `yaml:"max-paths" default:"50000"`
	MaxBytes             uint64                `yaml:"max-bytes" default:"5368709120"`
	EventRetention       string                `yaml:"event-retention" default:"30d"`
	EventMaxPerWorkspace int                   `yaml:"event-max-per-workspace" default:"100000"`
	BlobGCGrace          string                `yaml:"blob-gc-grace" default:"24h"`
	StagingTTL           string                `yaml:"staging-ttl" default:"24h"`
	PruneBatchSize       int                   `yaml:"prune-batch-size" default:"500"`
	MaxWorkspacesPerUser int                   `yaml:"max-workspaces-per-user" default:"8"`
	Roots                []WorkspaceRootConfig `yaml:"roots"`
}

func (c *WorkspaceConfig) Validate() error {
	if strings.TrimSpace(c.BlobPath) == "" {
		return fmt.Errorf("workspace blob-path must not be blank")
	}
	if strings.ContainsRune(c.BlobPath, '\x00') {
		return fmt.Errorf("workspace blob-path must not contain NUL")
	}
	if c.MaxPaths <= 0 || c.MaxPaths > maxWorkspacePaths {
		return fmt.Errorf("workspace max-paths must be between 1 and %d", maxWorkspacePaths)
	}
	if c.MaxBytes == 0 || c.MaxBytes > maxWorkspaceBytes {
		return fmt.Errorf("workspace max-bytes must be between 1 and %d", maxWorkspaceBytes)
	}
	if c.EventMaxPerWorkspace <= 0 {
		return fmt.Errorf("workspace event-max-per-workspace must be positive")
	}
	if err := validatePositiveWorkspaceDuration("event-retention", c.EventRetention); err != nil {
		return err
	}
	if err := validatePositiveWorkspaceDuration("blob-gc-grace", c.BlobGCGrace); err != nil {
		return err
	}
	if err := validatePositiveWorkspaceDuration("staging-ttl", c.StagingTTL); err != nil {
		return err
	}
	if c.PruneBatchSize <= 0 || c.PruneBatchSize > maxWorkspacePruneBatch {
		return fmt.Errorf("workspace prune-batch-size must be between 1 and %d", maxWorkspacePruneBatch)
	}
	if c.MaxWorkspacesPerUser != WorkspaceMaxPerUser {
		return fmt.Errorf("workspace max-workspaces-per-user must be %d", WorkspaceMaxPerUser)
	}
	seen := make(map[int64]map[string]struct{}, len(c.Roots))
	canonicalRoots := make(map[int64][]string, len(c.Roots))
	home, _ := os.UserHomeDir()
	for index, rootConfig := range c.Roots {
		if rootConfig.UID <= 0 {
			return fmt.Errorf("workspace roots[%d].uid must be positive", index)
		}
		if _, err := dto.ParseWorkspaceUUID("workspaceId", rootConfig.WorkspaceID); err != nil {
			return fmt.Errorf("workspace roots[%d].workspace-id is invalid", index)
		}
		if seen[rootConfig.UID] == nil {
			seen[rootConfig.UID] = make(map[string]struct{})
		}
		if _, exists := seen[rootConfig.UID][rootConfig.WorkspaceID]; exists {
			return fmt.Errorf("workspace roots[%d].workspace-id is duplicated", index)
		}
		seen[rootConfig.UID][rootConfig.WorkspaceID] = struct{}{}
		if len(seen[rootConfig.UID]) > WorkspaceMaxPerUser {
			return fmt.Errorf("workspace roots[%d].uid exceeds workspace cap", index)
		}
		root := rootConfig.Root
		if !filepath.IsAbs(root) || filepath.Clean(root) != root {
			return fmt.Errorf("workspace roots[%d].root must be a clean absolute directory", index)
		}
		if root == string(filepath.Separator) || (home != "" && root == filepath.Clean(home)) {
			return fmt.Errorf("workspace roots[%d].root is too broad", index)
		}
		info, err := os.Stat(root)
		if err != nil || !info.IsDir() {
			return fmt.Errorf("workspace roots[%d].root must be an existing directory", index)
		}
		canonical, err := filepath.EvalSymlinks(root)
		if err != nil || canonical != root {
			return fmt.Errorf("workspace roots[%d].root must not be a symlink", index)
		}
		for _, previous := range canonicalRoots[rootConfig.UID] {
			if workspaceRootsOverlap(previous, canonical) {
				return fmt.Errorf("workspace roots[%d].root overlaps another root", index)
			}
		}
		canonicalRoots[rootConfig.UID] = append(canonicalRoots[rootConfig.UID], canonical)
	}
	return nil
}

func workspaceRootsOverlap(left, right string) bool {
	if left == right {
		return true
	}
	for _, pair := range [][2]string{{left, right}, {right, left}} {
		rel, err := filepath.Rel(pair[0], pair[1])
		if err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			return true
		}
	}
	return false
}

func validatePositiveWorkspaceDuration(name, value string) error {
	duration, err := util.ParseDuration(value)
	if err != nil {
		return fmt.Errorf("workspace %s must be a valid duration: %w", name, err)
	}
	if duration <= 0 {
		return fmt.Errorf("workspace %s must be positive", name)
	}
	return nil
}
