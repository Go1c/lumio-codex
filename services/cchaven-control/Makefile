.PHONY: help run build test test-unit test-integration lint fmt db-up db-down admin tidy

# 用 docker-compose 的测试库跑集成测试；未启动容器时留空即回落到 embedded-postgres。
TEST_DB ?= postgres://cchaven:cchaven@localhost:5433/cchaven_test?sslmode=disable

help: ## 显示可用命令
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

run: ## 启动控制面服务（读取 .env）
	@set -a && [ -f .env ] && . ./.env; set +a; go run ./cmd/control

build: ## 编译二进制到 bin/
	go build -o bin/control ./cmd/control
	go build -o bin/admin-bootstrap ./cmd/admin-bootstrap

test: test-unit test-integration ## 跑全部测试

test-unit: ## 跑无需数据库的单元测试
	go test ./internal/...

test-integration: ## 跑需要 PostgreSQL 的集成与端到端测试
	CCHAVEN_TEST_DATABASE_URL="$(TEST_DB)" go test ./test/ -v

lint: ## 静态检查
	gofmt -l .
	go vet ./...

fmt: ## 格式化
	gofmt -w .

tidy: ## 整理依赖
	go mod tidy

db-up: ## 启动本地开发库与测试库
	docker compose up -d
	@echo "开发库 localhost:5432 / 测试库 localhost:5433"

db-down: ## 停止本地数据库
	docker compose down

admin: ## 创建首个管理员，需先设置 CCHAVEN_ADMIN_PASSWORD 与 EMAIL
	@set -a && [ -f .env ] && . ./.env; set +a; go run ./cmd/admin-bootstrap -email "$(EMAIL)"
