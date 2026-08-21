package cmd

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	internalApp "github.com/haierkeys/fast-note-sync-service/internal/app"
	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/dao"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	pkgapp "github.com/haierkeys/fast-note-sync-service/pkg/app"
	"github.com/haierkeys/fast-note-sync-service/pkg/logger"
	"github.com/spf13/cobra"
	"golang.org/x/sys/unix"
)

const bootstrapTokenLifetimeDays = 3650
const defaultBootstrapListenAddress = "127.0.0.1:9000"

type bootstrapWorkspaceOptions struct {
	ConfigPath    string
	TokenPath     string
	WorkspaceID   string
	WorkspaceRoot string
	ListenAddress string
	DefaultConfig string
}

type bootstrapWorkspaceResult struct {
	UID int64
}

func init() {
	var options bootstrapWorkspaceOptions
	command := &cobra.Command{
		Use:   "bootstrap-workspace",
		Short: "Issue a local workspace token and register its root",
		RunE: func(cmd *cobra.Command, args []string) error {
			options.DefaultConfig = configDefault
			result, err := bootstrapWorkspace(options)
			if err != nil {
				return err
			}
			fmt.Fprintf(cmd.OutOrStdout(), "BOOTSTRAP_UID=%d\n", result.UID)
			return nil
		},
	}
	flags := command.Flags()
	flags.StringVarP(&options.ConfigPath, "config", "c", "", "server config path")
	flags.StringVar(&options.TokenPath, "token-file", "", "private token output path")
	flags.StringVar(&options.WorkspaceID, "workspace-id", "", "workspace UUID")
	flags.StringVar(&options.WorkspaceRoot, "workspace-root", "", "absolute workspace root")
	flags.StringVar(&options.ListenAddress, "listen", defaultBootstrapListenAddress, "loopback server listen address")
	_ = command.MarkFlagRequired("config")
	_ = command.MarkFlagRequired("token-file")
	_ = command.MarkFlagRequired("workspace-id")
	_ = command.MarkFlagRequired("workspace-root")
	rootCmd.AddCommand(command)
}

func bootstrapWorkspace(options bootstrapWorkspaceOptions) (_ bootstrapWorkspaceResult, resultErr error) {
	if options.ConfigPath == "" || options.TokenPath == "" || options.WorkspaceID == "" || options.WorkspaceRoot == "" {
		return bootstrapWorkspaceResult{}, fmt.Errorf("config, token-file, workspace-id and workspace-root are required")
	}
	listenAddress, err := bootstrapListenAddress(options.ListenAddress)
	if err != nil {
		return bootstrapWorkspaceResult{}, err
	}
	root, err := filepath.Abs(options.WorkspaceRoot)
	if err != nil || filepath.Clean(root) != root {
		return bootstrapWorkspaceResult{}, fmt.Errorf("workspace-root must be a clean absolute path")
	}
	if info, statErr := os.Stat(root); statErr != nil || !info.IsDir() {
		return bootstrapWorkspaceResult{}, fmt.Errorf("workspace-root must be an existing directory")
	}
	canonical, err := filepath.EvalSymlinks(root)
	if err != nil || canonical != root {
		return bootstrapWorkspaceResult{}, fmt.Errorf("workspace-root must not be a symlink")
	}

	configPath, err := filepath.Abs(options.ConfigPath)
	if err != nil {
		return bootstrapWorkspaceResult{}, fmt.Errorf("resolve config path: %w", err)
	}
	workdir := filepath.Dir(filepath.Dir(configPath))
	if err := os.MkdirAll(workdir, 0700); err != nil {
		return bootstrapWorkspaceResult{}, fmt.Errorf("create working directory: %w", err)
	}
	if err := os.MkdirAll(filepath.Dir(configPath), 0700); err != nil {
		return bootstrapWorkspaceResult{}, fmt.Errorf("create config directory: %w", err)
	}
	lock, err := acquireBootstrapLock(configPath + ".lock")
	if err != nil {
		return bootstrapWorkspaceResult{}, err
	}
	defer func() {
		if err := releaseBootstrapLock(lock); resultErr == nil && err != nil {
			resultErr = err
		}
	}()
	previousWorkdir, err := os.Getwd()
	if err != nil {
		return bootstrapWorkspaceResult{}, fmt.Errorf("read working directory: %w", err)
	}
	if err := os.Chdir(workdir); err != nil {
		return bootstrapWorkspaceResult{}, fmt.Errorf("enter working directory: %w", err)
	}
	defer func() {
		if err := os.Chdir(previousWorkdir); resultErr == nil && err != nil {
			resultErr = fmt.Errorf("restore working directory: %w", err)
		}
	}()

	if err := ensureBootstrapConfig(configPath, options.DefaultConfig); err != nil {
		return bootstrapWorkspaceResult{}, err
	}
	appConfig, _, err := internalApp.LoadConfig(configPath)
	if err != nil {
		return bootstrapWorkspaceResult{}, fmt.Errorf("load server config: %w", err)
	}
	appConfig.Server.HttpPort = listenAddress
	appConfig.Server.PrivateHttpListen = ""
	appConfig.Server.WebGuiPort = ""
	appConfig.Server.SharePort = ""
	if err := savePrivateConfig(appConfig); err != nil {
		return bootstrapWorkspaceResult{}, err
	}

	lg, err := logger.NewLogger(logger.Config{
		Level:      appConfig.Log.Level,
		File:       appConfig.Log.File,
		Production: appConfig.Log.Production,
	})
	if err != nil {
		return bootstrapWorkspaceResult{}, fmt.Errorf("initialize logger: %w", err)
	}
	dbConfig := appConfig.Database
	dbConfig.RunMode = appConfig.Server.RunMode
	db, err := dao.NewEngine(dbConfig, lg)
	if err != nil {
		return bootstrapWorkspaceResult{}, fmt.Errorf("initialize database: %w", err)
	}
	application, err := internalApp.NewApp(appConfig, lg, db, frontendFiles)
	if err != nil {
		return bootstrapWorkspaceResult{}, fmt.Errorf("initialize application: %w", err)
	}
	defer func() {
		if err := application.Close(); resultErr == nil && err != nil {
			resultErr = fmt.Errorf("close application: %w", err)
		}
	}()

	ctx := context.Background()
	token, uid := reusableBootstrapToken(ctx, application, options.TokenPath)
	if token == "" {
		issued, issuedUID, issueErr := issueBootstrapToken(ctx, application)
		if issueErr != nil {
			return bootstrapWorkspaceResult{}, issueErr
		}
		token, uid = issued, issuedUID
		if err := writePrivateFile(options.TokenPath, []byte(token)); err != nil {
			return bootstrapWorkspaceResult{}, fmt.Errorf("write token: %w", err)
		}
	}

	upsertWorkspaceRoot(&appConfig.Workspace, config.WorkspaceRootConfig{
		UID:         uid,
		WorkspaceID: options.WorkspaceID,
		Root:        root,
	})
	if err := appConfig.Workspace.Validate(); err != nil {
		return bootstrapWorkspaceResult{}, fmt.Errorf("validate workspace config: %w", err)
	}
	if err := savePrivateConfig(appConfig); err != nil {
		return bootstrapWorkspaceResult{}, err
	}
	return bootstrapWorkspaceResult{UID: uid}, nil
}

func acquireBootstrapLock(path string) (*os.File, error) {
	lock, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return nil, fmt.Errorf("open bootstrap lock: %w", err)
	}
	if err := os.Chmod(path, 0600); err != nil {
		_ = lock.Close()
		return nil, fmt.Errorf("protect bootstrap lock: %w", err)
	}
	if err := unix.Flock(int(lock.Fd()), unix.LOCK_EX); err != nil {
		_ = lock.Close()
		return nil, fmt.Errorf("lock bootstrap state: %w", err)
	}
	return lock, nil
}

func releaseBootstrapLock(lock *os.File) error {
	if err := unix.Flock(int(lock.Fd()), unix.LOCK_UN); err != nil {
		_ = lock.Close()
		return fmt.Errorf("unlock bootstrap state: %w", err)
	}
	if err := lock.Close(); err != nil {
		return fmt.Errorf("close bootstrap lock: %w", err)
	}
	return nil
}

func bootstrapListenAddress(address string) (string, error) {
	if address == "" {
		address = defaultBootstrapListenAddress
	}
	host, portText, err := net.SplitHostPort(address)
	if err != nil {
		return "", fmt.Errorf("listen must be a loopback host:port")
	}
	ip := net.ParseIP(host)
	port, portErr := strconv.Atoi(portText)
	if ip == nil || !ip.IsLoopback() || portErr != nil || port < 1 || port > 65535 {
		return "", fmt.Errorf("listen must be a loopback host:port")
	}
	return net.JoinHostPort(ip.String(), strconv.Itoa(port)), nil
}

func ensureBootstrapConfig(path, source string) error {
	if _, err := os.Stat(path); err == nil {
		return nil
	} else if !os.IsNotExist(err) {
		return fmt.Errorf("inspect server config: %w", err)
	}
	if strings.TrimSpace(source) == "" {
		return fmt.Errorf("embedded server config is unavailable")
	}
	secret, err := randomHex(32)
	if err != nil {
		return fmt.Errorf("generate server secret: %w", err)
	}
	source = strings.Replace(source, "fast-note-sync-Auth-Token", secret, 1)
	if err := writePrivateFile(path, []byte(source)); err != nil {
		return fmt.Errorf("create server config: %w", err)
	}
	return nil
}

func reusableBootstrapToken(ctx context.Context, application *internalApp.App, path string) (string, int64) {
	bytes, err := os.ReadFile(path)
	if err != nil || len(bytes) == 0 || strings.TrimSpace(string(bytes)) != string(bytes) {
		return "", 0
	}
	claims, err := application.TokenManager.Parse(string(bytes))
	if err != nil || claims.UID <= 0 || claims.TokenID <= 0 {
		return "", 0
	}
	active, err := application.TokenService.GetActiveToken(ctx, claims.UID, claims.TokenID)
	if err != nil || !isReusableBootstrapToken(active, claims.Nonce) {
		return "", 0
	}
	return string(bytes), claims.UID
}

func isReusableBootstrapToken(active *domain.AuthToken, nonce string) bool {
	return active != nil && active.Status == 1 && active.ClientType == "fns-agent" &&
		nonce != "" && active.TokenString == nonce &&
		time.Now().Before(active.ExpiredAt) &&
		pkgapp.VerifyExactPermissions(active.Scope, "ws", "fns-agent", "workspace_rw")
}

func issueBootstrapToken(ctx context.Context, application *internalApp.App) (string, int64, error) {
	suffix, err := randomHex(6)
	if err != nil {
		return "", 0, fmt.Errorf("generate local identity: %w", err)
	}
	username := "bc_" + suffix
	password, err := randomHex(24)
	if err != nil {
		return "", 0, fmt.Errorf("generate local password: %w", err)
	}
	user, err := application.UserService.Create(ctx, &dto.UserCreateRequest{
		Email:           username + "@bestcodex.local",
		Username:        username,
		Password:        password,
		ConfirmPassword: password,
	})
	if err != nil {
		return "", 0, fmt.Errorf("create local workspace identity: %w", err)
	}
	issued, err := application.TokenService.Create(ctx, user.UID, &dto.TokenIssueRequest{
		ClientType:  "fns-agent",
		Protocol:    "ws",
		Client:      "fns-agent",
		Function:    "workspace_rw",
		ExpiredDays: bootstrapTokenLifetimeDays,
	})
	if err != nil {
		return "", 0, fmt.Errorf("issue workspace token: %w", err)
	}
	if _, err := pkgapp.ParseTokenWithKey(issued.TokenString, application.Config().Security.AuthTokenKey); err != nil {
		return "", 0, fmt.Errorf("verify issued workspace token: %w", err)
	}
	return issued.TokenString, user.UID, nil
}

func upsertWorkspaceRoot(workspace *config.WorkspaceConfig, next config.WorkspaceRootConfig) {
	kept := workspace.Roots[:0]
	for _, current := range workspace.Roots {
		if current.WorkspaceID == next.WorkspaceID || current.Root == next.Root {
			continue
		}
		kept = append(kept, current)
	}
	workspace.Roots = append(kept, next)
}

func savePrivateConfig(appConfig *internalApp.AppConfig) error {
	if err := appConfig.Save(); err != nil {
		return fmt.Errorf("save server config: %w", err)
	}
	if err := os.Chmod(appConfig.File, 0600); err != nil {
		return fmt.Errorf("protect server config: %w", err)
	}
	return nil
}

func writePrivateFile(path string, contents []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return err
	}
	temporary := path + ".tmp"
	if err := os.WriteFile(temporary, contents, 0600); err != nil {
		return err
	}
	if err := os.Chmod(temporary, 0600); err != nil {
		return err
	}
	return os.Rename(temporary, path)
}

func randomHex(bytes int) (string, error) {
	buffer := make([]byte, bytes)
	if _, err := rand.Read(buffer); err != nil {
		return "", err
	}
	return hex.EncodeToString(buffer), nil
}
