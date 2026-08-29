# Zen Proxy

轻量级、高性能的 OpenCode Zen 协议代理服务（基于 Rust + Axum + Tokio 构建）。

---

## 🌟 功能特性

- **多协议支持**：
  - `GET /v1/models`：获取全部可用模型列表
  - `POST /v1/chat/completions`：标准 OpenAI 对话接口（支持 SSE 实时流式传输）
  - `POST /v1/responses`：OpenAI Responses / Realtime 协议接口
  - `POST /v1/messages`：Anthropic 协议接口
  - `GET /api/status` & `GET /health`：健康检查接口
- **自动协议适配**：针对 `muse-spark-1.2-contributor-free` 等基于 `/responses` 协议的模型，若客户端通过标准 `/chat/completions` 发送 `messages`，代理会自动将其转换为 `input` 并在非流式下自动包装为标准 `chat.completion` 格式返回。
- **模型名清理**：自动剔除 `[1m]`、`[128k]` 等非标准模型后缀。
- **全量 Header 透传**：完整透传 `Authorization` / `x-api-key` / `api-key`，兼容各类商业模型调用。
- **跨平台与容器化**：提供 Dockerfile 与 GitHub Actions 自动化编译支持。

---

## 🚀 部署与使用方式

### 方式 1：Docker 部署（推荐）

#### 使用预构建镜像（来自 GitHub Actions）：
```bash
docker run -d \
  --name zen-proxy \
  -p 4096:4096 \
  --restart unless-stopped \
  ghcr.io/<你的GitHub用户名>/zen-proxy:latest
```

#### 本地构建 Docker 镜像：
```bash
cd zen-proxy
docker build -t zen-proxy .
docker run -d -p 4096:4096 --name zen-proxy zen-proxy
```

#### 使用 Docker Compose：
```yaml
version: '3.8'
services:
  zen-proxy:
    image: ghcr.io/<你的GitHub用户名>/zen-proxy:latest
    container_name: zen-proxy
    restart: unless-stopped
    ports:
      - "4096:4096"
    environment:
      - PORT=4096
      - HOST=0.0.0.0
      - RUST_LOG=info
```

---

### 方式 2：GitHub Actions 下载预编译二进制（本地运行）

推送到 GitHub 后，Actions 会自动构建出以下平台的免安装单文件可执行文件：
- **Windows**: `zen-proxy-windows-amd64.exe`
- **Linux**: `zen-proxy-linux-amd64`
- **macOS (Apple Silicon)**: `zen-proxy-macos-arm64`
- **macOS (Intel)**: `zen-proxy-macos-amd64`

在 Action 的 **Artifacts** 或 **Release** 中下载后直接双击 / 命令行运行即可：
```bash
./zen-proxy.exe
```

---

## ⚙️ 环境变量配置

| 变量名 | 默认值 | 说明 |
| :--- | :--- | :--- |
| `PORT` | `4096` | 监听端口 |
| `HOST` | `0.0.0.0` | 监听网卡地址 |
| `ZEN_BASE` | `https://opencode.ai/zen/v1` | 上游 OpenCode Zen 基础地址 |
| `ZEN_USER_AGENT` | `opencode/1.18.18` | 模拟客户端 User-Agent |
| `RUST_LOG` | `info` | 日志级别（`debug`/`info`/`warn`/`error`） |
