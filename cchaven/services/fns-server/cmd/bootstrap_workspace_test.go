package cmd

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	internalApp "github.com/haierkeys/fast-note-sync-service/internal/app"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	pkgapp "github.com/haierkeys/fast-note-sync-service/pkg/app"
)

func TestBootstrapWorkspaceIssuesReusableAgentToken(t *testing.T) {
	temp := t.TempDir()
	root := filepath.Join(temp, "workspace")
	if err := os.Mkdir(root, 0700); err != nil {
		t.Fatal(err)
	}
	root, err := filepath.EvalSymlinks(root)
	if err != nil {
		t.Fatal(err)
	}
	configPath := filepath.Join(temp, "config", "config.yaml")
	tokenPath := filepath.Join(temp, "state", "token")
	source, err := os.ReadFile(filepath.Join("..", "config", "config.yaml"))
	if err != nil {
		t.Fatal(err)
	}

	result, err := bootstrapWorkspace(bootstrapWorkspaceOptions{
		ConfigPath:    configPath,
		TokenPath:     tokenPath,
		WorkspaceID:   "80000000-0000-4000-8000-202608210001",
		WorkspaceRoot: root,
		ListenAddress: "127.0.0.1:19000",
		DefaultConfig: string(source),
	})
	if err != nil {
		t.Fatal(err)
	}
	if result.UID <= 0 {
		t.Fatalf("UID = %d, want positive", result.UID)
	}

	token, err := os.ReadFile(tokenPath)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Count(string(token), ".") != 2 {
		t.Fatalf("token is not a JWT: length=%d", len(token))
	}
	info, err := os.Stat(tokenPath)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0600 {
		t.Fatalf("token mode = %o, want 600", info.Mode().Perm())
	}

	config, _, err := internalApp.LoadConfig(configPath)
	if err != nil {
		t.Fatal(err)
	}
	if config.Server.HttpPort != "127.0.0.1:19000" {
		t.Fatalf("http-port = %q", config.Server.HttpPort)
	}
	if len(config.Workspace.Roots) != 1 {
		t.Fatalf("roots = %#v", config.Workspace.Roots)
	}
	registered := config.Workspace.Roots[0]
	if registered.UID != result.UID || registered.Root != root || registered.WorkspaceID != "80000000-0000-4000-8000-202608210001" {
		t.Fatalf("registered root = %#v", registered)
	}
	claims, err := pkgapp.ParseTokenWithKey(string(token), config.Security.AuthTokenKey)
	if err != nil {
		t.Fatal(err)
	}
	if claims.UID != result.UID || claims.TokenID <= 0 {
		t.Fatalf("claims = %#v", claims)
	}

	second, err := bootstrapWorkspace(bootstrapWorkspaceOptions{
		ConfigPath:    configPath,
		TokenPath:     tokenPath,
		WorkspaceID:   "80000000-0000-4000-8000-202608210001",
		WorkspaceRoot: root,
		ListenAddress: "127.0.0.1:19000",
		DefaultConfig: string(source),
	})
	if err != nil {
		t.Fatal(err)
	}
	if second.UID != result.UID {
		t.Fatalf("second UID = %d, want %d", second.UID, result.UID)
	}
	secondToken, err := os.ReadFile(tokenPath)
	if err != nil {
		t.Fatal(err)
	}
	if string(secondToken) != string(token) {
		t.Fatal("retry replaced an active token")
	}
}

func TestBootstrapListenAddressRejectsNonLoopback(t *testing.T) {
	if _, err := bootstrapListenAddress("0.0.0.0:9000"); err == nil {
		t.Fatal("public listen address was accepted")
	}
	if got, err := bootstrapListenAddress(""); err != nil || got != defaultBootstrapListenAddress {
		t.Fatalf("default listen = %q, %v", got, err)
	}
}

func TestReusableBootstrapTokenRequiresExactAgentScope(t *testing.T) {
	valid := &domain.AuthToken{
		Status:      1,
		ClientType:  "fns-agent",
		TokenString: "current-nonce",
		Scope:       "p:ws c:fns-agent f:workspace_rw",
		ExpiredAt:   time.Now().Add(time.Hour),
	}
	if !isReusableBootstrapToken(valid, "current-nonce") {
		t.Fatal("exact workspace token was rejected")
	}
	if isReusableBootstrapToken(valid, "rotated-nonce") {
		t.Fatal("rotated workspace token was reused")
	}
	valid.Scope = "p:ws c:fns-agent f:workspace_rw,note_rw"
	if isReusableBootstrapToken(valid, "current-nonce") {
		t.Fatal("token with extra permissions was reused")
	}
}

func TestBootstrapWorkspaceSerializesConcurrentConfigUpdates(t *testing.T) {
	temp := t.TempDir()
	configPath := filepath.Join(temp, "server", "config", "config.yaml")
	tokenPath := filepath.Join(temp, "state", "token")
	source, err := os.ReadFile(filepath.Join("..", "config", "config.yaml"))
	if err != nil {
		t.Fatal(err)
	}

	var ready sync.WaitGroup
	ready.Add(2)
	start := make(chan struct{})
	errors := make(chan error, 2)
	for index := 1; index <= 2; index++ {
		index := index
		root := filepath.Join(temp, fmt.Sprintf("workspace-%d", index))
		if err := os.Mkdir(root, 0700); err != nil {
			t.Fatal(err)
		}
		root, err = filepath.EvalSymlinks(root)
		if err != nil {
			t.Fatal(err)
		}
		go func() {
			ready.Done()
			<-start
			_, runErr := bootstrapWorkspace(bootstrapWorkspaceOptions{
				ConfigPath:    configPath,
				TokenPath:     tokenPath,
				WorkspaceID:   fmt.Sprintf("80000000-0000-4000-8000-20260821000%d", index),
				WorkspaceRoot: root,
				ListenAddress: "127.0.0.1:19000",
				DefaultConfig: string(source),
			})
			errors <- runErr
		}()
	}
	ready.Wait()
	close(start)
	for index := 0; index < 2; index++ {
		if err := <-errors; err != nil {
			t.Fatal(err)
		}
	}

	appConfig, _, err := internalApp.LoadConfig(configPath)
	if err != nil {
		t.Fatal(err)
	}
	if len(appConfig.Workspace.Roots) != 2 {
		t.Fatalf("roots = %#v, want both concurrent updates", appConfig.Workspace.Roots)
	}
}
