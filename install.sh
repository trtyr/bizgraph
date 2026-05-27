#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="${HOME}/.local/bin"
BIN_NAME="bizgraph"
PROJECT_ROOT="$(cd "$(dirname "$0")" && pwd)"

echo "=== BizGraph Install ==="

mkdir -p "$INSTALL_DIR"

echo "[1/3] Building release..."
cargo build --release

echo "[2/3] Installing to $INSTALL_DIR ..."
cp "$PROJECT_ROOT/target/release/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
chmod +x "$INSTALL_DIR/$BIN_NAME"

echo "[3/3] Done."
echo ""
echo "Installed:"
echo "  $INSTALL_DIR/$BIN_NAME"
echo ""

case ":$PATH:" in
    *":$INSTALL_DIR:"*)
        echo "$INSTALL_DIR is in PATH"
        ;;
    *)
        echo "$INSTALL_DIR not in PATH."
        echo "Add this to your ~/.zshrc or ~/.bashrc:"
        echo ""
        echo 'export PATH="$HOME/.local/bin:$PATH"'
        ;;
esac

echo "Try: bizgraph --help"
