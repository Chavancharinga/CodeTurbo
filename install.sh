#!/bin/sh
# Instala o CodeTurbo no Linux e no macOS:
#   curl -fsSL https://raw.githubusercontent.com/Chavancharinga/CodeTurbo/main/install.sh | sh
set -eu

repo="Chavancharinga/CodeTurbo"
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64 | Linux-amd64) target=x86_64-unknown-linux-musl ;;
    Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
    Darwin-arm64) target=aarch64-apple-darwin ;;
    Darwin-x86_64) target=x86_64-apple-darwin ;;
    *)
        echo "Não há executável pronto para $(uname -s) $(uname -m)." >&2
        echo "Instale a partir do código-fonte: cargo install --locked --git https://github.com/$repo coturbo-cli" >&2
        exit 1
        ;;
esac

dir="${CODETURBO_INSTALL_DIR:-$HOME/.local/bin}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Baixando o CodeTurbo ($target)..."
curl -fsSL "https://github.com/$repo/releases/latest/download/codeturbo-$target.tar.gz" | tar -xz -C "$tmp"
mkdir -p "$dir"
cp "$tmp/codeturbo" "$dir/codeturbo.new"
chmod 755 "$dir/codeturbo.new"
mv -f "$dir/codeturbo.new" "$dir/codeturbo"

echo "CodeTurbo instalado em $dir/codeturbo"
case ":$PATH:" in
    *":$dir:"*) echo "Para começar, execute: codeturbo" ;;
    *)
        echo "Adicione $dir ao PATH para usar o comando codeturbo, por exemplo:"
        echo "  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.profile && . ~/.profile"
        ;;
esac
