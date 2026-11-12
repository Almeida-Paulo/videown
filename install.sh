#!/usr/bin/env bash
# Instala ou atualiza o Videown num servidor Ubuntu ou Debian (x86_64 ou ARM64).
# Uso, dentro da pasta do projeto:  sudo ./install.sh
# Para usar outra porta interna na primeira instalação:  sudo VIDEOWN_PORT=8090 ./install.sh
# Rodar de novo atualiza o Videown, o yt-dlp, o Deno e o gerador de token, sem mexer na configuração nem nos arquivos.
set -euo pipefail

if [ "$(id -u)" != 0 ]; then
  echo "Rode com sudo: sudo ./install.sh" >&2
  exit 1
fi
cd "$(dirname "$0")"

PREFIX=/opt/videown
DATA=/var/lib/videown
BIN=$DATA/bin
ENV_FILE=/etc/videown.env
PORT=${VIDEOWN_PORT:-8080}
POT_PORT=4416

# Gerador de token PO (github.com/jim60105/bgutil-ytdlp-pot-provider-rs), versão fixa e conferida.
POT_VERSION=v0.8.1
POT_PLUGIN_SHA256=99fd83b98fa93b193d6a3b69dc74410d76e7a2b889868c54d16121cac9060344

case "$(uname -m)" in
  x86_64)
    YTDLP_ASSET=yt-dlp_linux; DENO_ASSET=deno-x86_64-unknown-linux-gnu.zip
    POT_ASSET=bgutil-pot-linux-x86_64
    POT_SHA256=e7c264a574fa2705b6e5dc62283a8a4e80130f27b9d7e9df44e6b09aa6151a87 ;;
  aarch64)
    YTDLP_ASSET=yt-dlp_linux_aarch64; DENO_ASSET=deno-aarch64-unknown-linux-gnu.zip
    POT_ASSET=bgutil-pot-linux-aarch64
    POT_SHA256=4f4a1f681dba45e695e1c14d314517da180a1fd374afd09d634fd80ef6d0284b ;;
  *) echo "Processador não suportado: $(uname -m)" >&2; exit 1 ;;
esac

step() { printf '\n==> %s\n' "$1"; }
check_sha256() {
  if [ "$(sha256sum "$1" | cut -d' ' -f1)" != "$2" ]; then
    echo "O checksum de $(basename "$1") não confere. Nada foi instalado." >&2
    exit 1
  fi
}

step "Instalando pacotes do sistema (ffmpeg, unzip, compilador)"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ffmpeg unzip curl ca-certificates build-essential >/dev/null

step "Compilando o Videown"
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null; then
  echo "Rust não encontrado. Instalando com rustup…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  export PATH="$HOME/.cargo/bin:$PATH"
fi
cargo build --release
install -Dm755 target/release/videown "$PREFIX/videown"

step "Criando o usuário e as pastas"
id videown >/dev/null 2>&1 || useradd --system --home-dir "$DATA" --shell /usr/sbin/nologin videown
install -d -o videown -g videown -m 750 "$DATA" "$BIN" "$DATA/deno"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
latest_tag() { curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$1/releases/latest" | sed 's#.*/tag/##'; }

step "Baixando o yt-dlp e conferindo o checksum"
TAG=$(latest_tag yt-dlp/yt-dlp)
BASE="https://github.com/yt-dlp/yt-dlp/releases/download/$TAG"
curl -fsSL -o "$TMP/$YTDLP_ASSET" "$BASE/$YTDLP_ASSET"
curl -fsSL -o "$TMP/SUMS" "$BASE/SHA2-256SUMS"
(cd "$TMP" && grep " $YTDLP_ASSET\$" SUMS | sha256sum -c --quiet -)
install -o videown -g videown -m 755 "$TMP/$YTDLP_ASSET" "$BIN/yt-dlp"
echo "yt-dlp $TAG"

step "Baixando o Deno (o YouTube exige um motor de JavaScript) e conferindo o checksum"
TAG=$(latest_tag denoland/deno)
BASE="https://github.com/denoland/deno/releases/download/$TAG"
curl -fsSL -o "$TMP/$DENO_ASSET" "$BASE/$DENO_ASSET"
curl -fsSL -o "$TMP/deno.sha256" "$BASE/$DENO_ASSET.sha256sum"
EXPECTED=$(grep -m1 -oiE '[0-9a-f]{64}' "$TMP/deno.sha256" | tr 'A-F' 'a-f')
ACTUAL=$(sha256sum "$TMP/$DENO_ASSET" | cut -d' ' -f1)
if [ -z "$EXPECTED" ] || [ "$EXPECTED" != "$ACTUAL" ]; then
  echo "O checksum do Deno não confere. Nada foi instalado." >&2
  exit 1
fi
unzip -oq "$TMP/$DENO_ASSET" -d "$TMP"
install -o videown -g videown -m 755 "$TMP/deno" "$BIN/deno"
echo "Deno $TAG"

step "Baixando o gerador de token PO ($POT_VERSION) e conferindo o checksum"
BASE="https://github.com/jim60105/bgutil-ytdlp-pot-provider-rs/releases/download/$POT_VERSION"
curl -fsSL -o "$TMP/$POT_ASSET" "$BASE/$POT_ASSET"
curl -fsSL -o "$TMP/pot-plugin.zip" "$BASE/bgutil-ytdlp-pot-provider-rs.zip"
check_sha256 "$TMP/$POT_ASSET" "$POT_SHA256"
check_sha256 "$TMP/pot-plugin.zip" "$POT_PLUGIN_SHA256"
install -o videown -g videown -m 755 "$TMP/$POT_ASSET" "$BIN/bgutil-pot"
# O yt-dlp carrega plugins da pasta yt-dlp-plugins que fica ao lado do executável.
PLUGIN_DIR="$BIN/yt-dlp-plugins/bgutil-ytdlp-pot-provider-rs"
rm -rf "$PLUGIN_DIR"
mkdir -p "$PLUGIN_DIR"
unzip -oq "$TMP/pot-plugin.zip" -d "$PLUGIN_DIR"
chown -R videown:videown "$BIN/yt-dlp-plugins"
install -d -o videown -g videown -m 750 "$DATA/pot"

NEW_PASSWORD=""
if [ ! -f "$ENV_FILE" ]; then
  step "Criando a configuração em $ENV_FILE"
  NEW_PASSWORD=$(head -c 48 /dev/urandom | base64 -w0 | tr -dc 'A-Za-z0-9')
  NEW_PASSWORD=${NEW_PASSWORD:0:24}
  cat > "$ENV_FILE" <<EOF
# Configuração do Videown. Depois de mudar, rode: sudo systemctl restart videown
VIDEOWN_PASSWORD=$NEW_PASSWORD
VIDEOWN_LISTEN=127.0.0.1:$PORT
VIDEOWN_DATA_DIR=$DATA
VIDEOWN_YTDLP=$BIN/yt-dlp
VIDEOWN_RETENTION_HOURS=72
VIDEOWN_MAX_PARALLEL=2
VIDEOWN_AUTO_UPDATE=1
# Tamanho máximo de cada arquivo (ex.: 500M, 4G). Sem limite se ficar comentado.
#VIDEOWN_MAX_FILESIZE=4G
# Cookies para o YouTube não bloquear o servidor. Veja o README.
#VIDEOWN_COOKIES_FILE=$DATA/cookies.txt
EOF
  chmod 600 "$ENV_FILE"
fi

# Opções de rede, comentadas. Também entram em instalações feitas antes delas existirem.
if ! grep -q "VIDEOWN_FORCE_IPV4" "$ENV_FILE"; then
  cat >> "$ENV_FILE" <<'EOF'

# Rede do yt-dlp. Veja "O YouTube bloqueou o servidor" no README.
#VIDEOWN_FORCE_IPV4=1
#VIDEOWN_PROXY=socks5://127.0.0.1:1080
#VIDEOWN_EXTRACTOR_ARGS=youtube:player_client=default,mweb
EOF
fi
LISTEN=$(grep -E '^VIDEOWN_LISTEN=' "$ENV_FILE" | cut -d= -f2-)

step "Ligando o gerador de token PO em 127.0.0.1:$POT_PORT"
if ss -Htln "sport = :$POT_PORT" | grep -q . && ! systemctl is-active --quiet videown-pot; then
  echo "A porta $POT_PORT já está em uso por outro programa. O gerador de token não foi ligado." >&2
else
  install -m 644 deploy/videown-pot.service /etc/systemd/system/videown-pot.service
  systemctl daemon-reload
  systemctl enable videown-pot >/dev/null 2>&1
  systemctl restart videown-pot
fi

step "Ligando o serviço"
install -m 644 deploy/videown.service /etc/systemd/system/videown.service
systemctl daemon-reload
systemctl enable videown >/dev/null 2>&1
systemctl restart videown
sleep 1
if systemctl is-active --quiet videown; then
  echo "Videown rodando em $LISTEN."
else
  echo "O serviço não subiu. Veja: journalctl -u videown -n 50" >&2
  echo "Se a porta já estiver em uso, troque VIDEOWN_LISTEN em $ENV_FILE e rode: sudo systemctl restart videown" >&2
  exit 1
fi

if [ -n "$NEW_PASSWORD" ]; then
  printf '\nSua senha de acesso: %s\n(Ela fica em %s. Para trocar, edite o arquivo e reinicie o serviço.)\n' "$NEW_PASSWORD" "$ENV_FILE"
fi
printf '\nPróximo passo: publicar o site com HTTPS por um proxy reverso (veja o README).\n'
