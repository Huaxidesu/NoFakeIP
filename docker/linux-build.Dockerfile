# 本地构建 Linux AppImage（Docker 方案）
#
# 为什么需要：Tauri 的 Linux 目标依赖 webkit2gtk 等系统库，无法在 Windows 上交叉编译。
# 本机 Docker Desktop 守护进程默认未启动；启动后可用本文件产出 AppImage。
#
# 用法（在仓库根目录）：
#   docker build -f docker/linux-build.Dockerfile -t nofakeip-linux .
#   docker create --name nfip nofakeip-linux
#   docker cp nfip:/app/desktop/src-tauri/target/release/bundle/appimage/. ./output/linux/
#   docker rm nfip
#
# 基础镜像刻意选 Ubuntu 22.04（glibc 2.35）而非 24.04：
# AppImage 的兼容性受构建机 glibc 版本制约，越老向上兼容越好。

FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_TERM_COLOR=always \
    PNPM_HOME=/usr/local/bin

# 1) 系统依赖：Tauri Linux 构建所需 + Rust/Node 工具链的基础包
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        curl wget file ca-certificates git pkg-config \
        libwebkit2gtk-4.1-dev \
        libappindicator3-dev \
        librsvg2-dev \
        patchelf \
        libxdo-dev \
        libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# 2) Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable --profile minimal
ENV PATH=/root/.cargo/bin:$PATH

# 3) Node + pnpm（版本与本地一致：Node 22 / pnpm 10）
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/* \
    && corepack enable && corepack prepare pnpm@10 --activate

WORKDIR /app

# 4) 先只拷贝依赖清单，让依赖层可缓存
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY desktop/package.json desktop/pnpm-lock.yaml ./desktop/
COPY desktop/src-tauri/Cargo.toml ./desktop/src-tauri/

RUN cd desktop && pnpm install --frozen-lockfile

# 5) 再拷贝源码并构建
COPY desktop ./desktop
COPY tools ./tools

RUN cd desktop && pnpm tauri build --bundles appimage

# 产物：/app/desktop/src-tauri/target/release/bundle/appimage/*.AppImage
CMD ["bash"]
