#!/usr/bin/env bash
# =============================================================================
# OpenIntentOS Installer
# =============================================================================
# Detects OS/arch, downloads the correct binary, and sets up the system.
# Safe to run multiple times (idempotent).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/OpenIntentOS/OpenIntentOS/main/deploy/install.sh | bash
#   # or
#   ./deploy/install.sh
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
REPO="OpenIntentOS/OpenIntentOS"
INSTALL_DIR="/usr/local/bin"
DATA_DIR="/var/lib/openintent"
CONFIG_DIR="/etc/openintent"
SERVICE_USER="openintent"
SERVICE_GROUP="openintent"
BINARY_NAME="openintent"
VERSION="${OPENINTENT_VERSION:-latest}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()  { printf "\033[1;34m[INFO]\033[0m  %s\n" "$*"; }
warn()  { printf "\033[1;33m[WARN]\033[0m  %s\n" "$*"; }
error() { printf "\033[1;31m[ERROR]\033[0m %s\n" "$*" >&2; exit 1; }

need_cmd() {
    if ! command -v "$1" &>/dev/null; then
        error "Required command not found: $1"
    fi
}

# ---------------------------------------------------------------------------
# Detect platform
# ---------------------------------------------------------------------------
detect_platform() {
    local os arch

    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)  os="unknown-linux-musl" ;;
        Darwin) os="apple-darwin" ;;
        *)      error "Unsupported OS: $os" ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch="x86_64" ;;
        aarch64|arm64)   arch="aarch64" ;;
        *)               error "Unsupported architecture: $arch" ;;
    esac

    PLATFORM="${arch}-${os}"
    info "Detected platform: ${PLATFORM}"
}

# ---------------------------------------------------------------------------
# Resolve download URL
# ---------------------------------------------------------------------------
resolve_url() {
    need_cmd curl

    if [ "$VERSION" = "latest" ]; then
        DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${BINARY_NAME}-${PLATFORM}"
    else
        DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${BINARY_NAME}-${PLATFORM}"
    fi

    info "Download URL: ${DOWNLOAD_URL}"
}

# ---------------------------------------------------------------------------
# Download and install binary
# ---------------------------------------------------------------------------
install_binary() {
    local tmp
    tmp="$(mktemp)"

    info "Downloading ${BINARY_NAME}..."
    if ! curl -fSL -o "$tmp" "$DOWNLOAD_URL"; then
        rm -f "$tmp"
        error "Download failed. Check that the release exists at: ${DOWNLOAD_URL}"
    fi

    chmod +x "$tmp"
    sudo mv "$tmp" "${INSTALL_DIR}/${BINARY_NAME}"
    info "Installed ${BINARY_NAME} to ${INSTALL_DIR}/${BINARY_NAME}"
}

# ---------------------------------------------------------------------------
# Create directories
# ---------------------------------------------------------------------------
create_directories() {
    info "Creating directories..."

    sudo mkdir -p "${DATA_DIR}/data"
    sudo mkdir -p "${DATA_DIR}/config"
    sudo mkdir -p "${CONFIG_DIR}"

    # Copy default config files if they exist alongside the installer
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local project_root="${script_dir}/.."

    if [ -d "${project_root}/config" ]; then
        info "Copying config files..."
        sudo cp -n "${project_root}/config/"* "${CONFIG_DIR}/" 2>/dev/null || true
    fi
}

# ---------------------------------------------------------------------------
# Create system user
# ---------------------------------------------------------------------------
create_user() {
    if id "${SERVICE_USER}" &>/dev/null; then
        info "User ${SERVICE_USER} already exists."
        return
    fi

    info "Creating system user: ${SERVICE_USER}"

    if command -v useradd &>/dev/null; then
        # Linux
        sudo useradd --system --no-create-home --shell /usr/sbin/nologin \
            --user-group "${SERVICE_USER}"
    elif command -v dscl &>/dev/null; then
        # macOS -- skip system user creation, launchd runs as current user
        warn "macOS detected. Skipping system user creation."
        return
    else
        error "Cannot create system user: no useradd or dscl found."
    fi
}

# ---------------------------------------------------------------------------
# Set permissions
# ---------------------------------------------------------------------------
set_permissions() {
    info "Setting ownership..."
    if id "${SERVICE_USER}" &>/dev/null; then
        sudo chown -R "${SERVICE_USER}:${SERVICE_GROUP}" "${DATA_DIR}"
    fi
}

# ---------------------------------------------------------------------------
# Install systemd service
# ---------------------------------------------------------------------------
install_service() {
    if ! command -v systemctl &>/dev/null; then
        warn "systemd not found. Skipping service installation."
        warn "You can run OpenIntentOS manually: ${INSTALL_DIR}/${BINARY_NAME} run --serve"
        return
    fi

    local service_src
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    service_src="${script_dir}/openintent.service"

    if [ ! -f "$service_src" ]; then
        warn "Service file not found at ${service_src}. Skipping service install."
        return
    fi

    info "Installing systemd service..."
    sudo cp "$service_src" /etc/systemd/system/openintent.service
    sudo systemctl daemon-reload
    sudo systemctl enable openintent.service

    info "Systemd service installed and enabled."
}

# ---------------------------------------------------------------------------
# Print next steps
# ---------------------------------------------------------------------------
print_instructions() {
    printf "\n"
    info "========================================="
    info " OpenIntentOS installed successfully!"
    info "========================================="
    printf "\n"
    printf "  Next steps:\n"
    printf "\n"
    printf "  1. Set your Anthropic API key:\n"
    printf "     sudo tee /etc/openintent/env <<< 'ANTHROPIC_API_KEY=sk-ant-your-key-here'\n"
    printf "     sudo chmod 600 /etc/openintent/env\n"
    printf "\n"

    if command -v systemctl &>/dev/null; then
        printf "  2. Start the service:\n"
        printf "     sudo systemctl start openintent\n"
        printf "\n"
        printf "  3. Check status:\n"
        printf "     sudo systemctl status openintent\n"
        printf "     journalctl -u openintent -f\n"
    else
        printf "  2. Start OpenIntentOS:\n"
        printf "     openintent run --serve\n"
    fi

    printf "\n"
    printf "  OpenIntentOS will be available at http://localhost:3000\n"
    printf "\n"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    info "OpenIntentOS Installer"
    printf "\n"

    detect_platform
    resolve_url
    install_binary
    create_directories
    create_user
    set_permissions
    install_service
    print_instructions
}

main "$@"
