#!/bin/sh
# Installs CodeTurbo on Linux, macOS and Android (Termux):
#   curl -fsSL https://raw.githubusercontent.com/Chavancharinga/CodeTurbo/main/install.sh | sh
set -eu

repo="Chavancharinga/CodeTurbo"
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64 | Linux-amd64) target=x86_64-unknown-linux-musl ;;
    Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
    Darwin-arm64) target=aarch64-apple-darwin ;;
    Darwin-x86_64) target=x86_64-apple-darwin ;;
    *)
        echo "There is no prebuilt binary for $(uname -s) $(uname -m)." >&2
        echo "Install from source: cargo install --locked --git https://github.com/$repo coturbo-cli" >&2
        exit 1
        ;;
esac

# Termux keeps its programs in $PREFIX/bin, which is always on its PATH.
termux=""
case "${PREFIX:-}" in
    *com.termux*) termux=1 ;;
esac
if [ -n "${CODETURBO_INSTALL_DIR:-}" ]; then
    dir="$CODETURBO_INSTALL_DIR"
elif [ -n "$termux" ]; then
    dir="$PREFIX/bin"
else
    dir="$HOME/.local/bin"
fi
tmp="$(mktemp -d "${TMPDIR:-/tmp}/codeturbo.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading CodeTurbo ($target)..."
curl -fsSL "https://github.com/$repo/releases/latest/download/codeturbo-$target.tar.gz" | tar -xz -C "$tmp"
mkdir -p "$dir"
cp "$tmp/codeturbo" "$dir/codeturbo.new"
chmod 755 "$dir/codeturbo.new"
mv -f "$dir/codeturbo.new" "$dir/codeturbo"
echo "CodeTurbo installed at $dir/codeturbo"

case ":$PATH:" in
    *":$dir:"*)
        echo "To start, run: codeturbo"
        ;;
    *)
        # Put the folder on the PATH of future shells.
        line="export PATH=\"$dir:\$PATH\""
        for rc in "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do
            case "$rc" in
                */.profile) ;;
                *) [ -f "$rc" ] || continue ;;
            esac
            grep -qsF "$line" "$rc" || printf '\n%s\n' "$line" >> "$rc"
        done
        echo "Added $dir to your PATH. Open a new terminal (or run: . ~/.profile), then run: codeturbo"
        ;;
esac
