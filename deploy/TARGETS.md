# OpenIntentOS Build Targets

Supported cross-compilation targets for OpenIntentOS. All Linux targets use musl for fully static binaries with zero runtime dependencies.

## Supported Targets

| Target Triple | Platform | Use Case |
|---|---|---|
| `x86_64-unknown-linux-musl` | Linux x86_64 | VPS, Docker, most cloud servers |
| `aarch64-unknown-linux-musl` | Linux ARM64 | ARM VPS, Raspberry Pi, Oracle Cloud free tier |
| `aarch64-unknown-linux-gnu` | Linux ARM64 (glibc) | Android via Termux |
| `x86_64-apple-darwin` | macOS Intel | MacBook Pro/Air (pre-2020) |
| `aarch64-apple-darwin` | macOS Apple Silicon | MacBook Pro/Air (M1+), Mac Mini, Mac Studio |
| `x86_64-pc-windows-msvc` | Windows x86_64 | Windows desktop/server |

## Build Commands

### Prerequisites

Install the target toolchain before building:

```bash
rustup target add <target-triple>
```

For Linux musl targets on non-Linux hosts, you also need a musl cross-compiler. On macOS:

```bash
brew install filosottile/musl-cross/musl-cross
```

### x86_64-unknown-linux-musl (VPS, Docker)

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
# Binary: target/x86_64-unknown-linux-musl/release/openintent
```

### aarch64-unknown-linux-musl (ARM VPS, Raspberry Pi)

```bash
rustup target add aarch64-unknown-linux-musl
# Requires aarch64-linux-musl-gcc cross-compiler
# On Ubuntu: sudo apt install gcc-aarch64-linux-gnu musl-tools
# On macOS:  brew install filosottile/musl-cross/musl-cross --with-aarch64
export CC_aarch64_unknown_linux_musl=aarch64-linux-musl-gcc
cargo build --release --target aarch64-unknown-linux-musl
# Binary: target/aarch64-unknown-linux-musl/release/openintent
```

### aarch64-unknown-linux-gnu (Android via Termux)

```bash
rustup target add aarch64-unknown-linux-gnu
# Requires Android NDK or aarch64-linux-gnu-gcc
# On Ubuntu: sudo apt install gcc-aarch64-linux-gnu
export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
cargo build --release --target aarch64-unknown-linux-gnu
# Binary: target/aarch64-unknown-linux-gnu/release/openintent
```

### x86_64-apple-darwin (macOS Intel)

```bash
# Native build on Intel Mac:
cargo build --release
# Cross-compile from Apple Silicon:
rustup target add x86_64-apple-darwin
cargo build --release --target x86_64-apple-darwin
# Binary: target/x86_64-apple-darwin/release/openintent
```

### aarch64-apple-darwin (macOS Apple Silicon)

```bash
# Native build on Apple Silicon:
cargo build --release
# Cross-compile from Intel Mac:
rustup target add aarch64-apple-darwin
cargo build --release --target aarch64-apple-darwin
# Binary: target/aarch64-apple-darwin/release/openintent
```

### x86_64-pc-windows-msvc (Windows)

```bash
# Native build on Windows (requires Visual Studio Build Tools):
cargo build --release
# Cross-compile from Linux (requires mingw):
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
# Binary: target/x86_64-pc-windows-gnu/release/openintent.exe
```

## Universal Binary (macOS)

To create a universal binary that runs on both Intel and Apple Silicon Macs:

```bash
rustup target add x86_64-apple-darwin aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
lipo -create \
  target/x86_64-apple-darwin/release/openintent \
  target/aarch64-apple-darwin/release/openintent \
  -output target/openintent-universal
```

## Docker Multi-Architecture

Build Docker images for multiple architectures using buildx:

```bash
docker buildx create --use
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -t openintentos/openintentos:latest \
  --push .
```

## Release Profile

The workspace `Cargo.toml` already includes an optimized release profile:

```toml
[profile.release]
lto = true          # Link-time optimization
codegen-units = 1   # Single codegen unit for better optimization
strip = true        # Strip debug symbols
panic = "abort"     # Smaller binary, no unwinding
opt-level = 3       # Maximum optimization
```

This produces small, fast binaries suitable for deployment.
