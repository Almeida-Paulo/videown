# Videown

**Seu próprio site para baixar vídeos e áudios.**

O Videown roda num servidor Linux e é usado pelo navegador, no celular ou no computador. Você cola um link, vê o
que tem nele, escolhe o formato e o servidor baixa com o [yt-dlp](https://github.com/yt-dlp/yt-dlp). O arquivo fica
disponível por um tempo e depois é apagado sozinho.

Software livre, sem anúncios, sem rastreamento e sem conta em serviço nenhum.

## Recursos

- Prévia do link antes de baixar: título, canal, duração e miniatura
- Playlists: mostra os itens e deixa escolher quais baixar
- Vídeo em MP4 (melhor qualidade, 1080p, 720p ou 480p) ou áudio em MP3 ou M4A
- Fila de downloads com progresso ao vivo e cancelamento
- Arquivos apagados automaticamente depois de um prazo (72 horas por padrão)
- Retoma sozinho o que estava baixando se o servidor reiniciar
- Acesso protegido por senha
- Página leve, com tema claro e escuro, feita para celular
- Milhares de sites, por meio do yt-dlp
- Gerador de token PO incluído, para reduzir os bloqueios do YouTube em servidores de nuvem

O Videown é um único binário em Rust, sem dependências. Ele tem cerca de 600 KB e usa poucos MB de memória parado.
O consumo real aparece durante os downloads, que ficam por conta do yt-dlp e do FFmpeg, e no gerador de token, um
serviço à parte.

## Requisitos

- Servidor Linux com Ubuntu 22.04 ou mais novo, ou Debian 12 ou mais novo, em x86_64 ou ARM64
- Acesso de administrador (`sudo`)
- Um domínio ou subdomínio para acessar com HTTPS

## Instalação

```bash
git clone https://github.com/Almeida-Paulo/videown.git
cd videown
sudo ./install.sh
```

O instalador:
- instala o FFmpeg e, se faltar, o Rust;
- compila o Videown;
- baixa o yt-dlp e o Deno, conferindo o checksum publicado por cada projeto;
- baixa o gerador de token PO ([bgutil-ytdlp-pot-provider-rs](https://github.com/jim60105/bgutil-ytdlp-pot-provider-rs))
  numa versão fixa, conferindo o checksum gravado no próprio instalador;
- cria o usuário `videown`, a configuração em `/etc/videown.env` e os serviços `videown` e `videown-pot` do systemd.

No final, ele mostra a senha de acesso.

O Videown escuta só em `127.0.0.1:8080`, então não fica exposto na internet. Se essa porta já estiver em uso, escolha
outra na primeira instalação: `sudo VIDEOWN_PORT=8090 ./install.sh`. O gerador de token usa `127.0.0.1:4416`. Para publicar o site, coloque na frente
um proxy reverso com HTTPS:

- **Cloudflare Tunnel:** aponte o hostname público para `http://localhost:8080`. Não é preciso abrir portas no
  servidor.
- **Caddy:** veja [deploy/Caddyfile.example](deploy/Caddyfile.example). O Caddy obtém o certificado sozinho, mas as
  portas 80 e 443 precisam estar abertas.
- **Nginx:** veja [deploy/nginx.conf.example](deploy/nginx.conf.example).

Para atualizar o Videown, o yt-dlp, o Deno e o gerador de token, rode `git pull` e depois `sudo ./install.sh` de novo. A configuração e
os arquivos são mantidos.

## Configuração

Tudo fica em `/etc/videown.env`. Depois de mudar algo, rode `sudo systemctl restart videown`.

| Variável | Padrão | O que faz |
|---|---|---|
| `VIDEOWN_PASSWORD` | gerada na instalação | Senha de acesso, com pelo menos 12 caracteres |
| `VIDEOWN_RETENTION_HOURS` | `72` | Horas até um arquivo ser apagado |
| `VIDEOWN_MAX_PARALLEL` | `2` | Downloads ao mesmo tempo |
| `VIDEOWN_MAX_FILESIZE` | sem limite | Tamanho máximo de cada arquivo, como `500M` ou `4G` |
| `VIDEOWN_COOKIES_FILE` | nenhum | Arquivo de cookies para sites que pedem login |
| `VIDEOWN_AUTO_UPDATE` | `1` | Atualiza o yt-dlp uma vez por dia (`0` desliga) |
| `VIDEOWN_LISTEN` | `127.0.0.1:8080` | Endereço e porta internos |
| `VIDEOWN_DATA_DIR` | `/var/lib/videown` | Onde ficam os arquivos e a fila |
| `VIDEOWN_YTDLP` | `/var/lib/videown/bin/yt-dlp` | Caminho do yt-dlp |
| `VIDEOWN_FORCE_IPV4` | `0` | Faz o yt-dlp usar só IPv4 (`1` liga) |
| `VIDEOWN_PROXY` | nenhum | Proxy para o yt-dlp, como `socks5://127.0.0.1:1080` |
| `VIDEOWN_EXTRACTOR_ARGS` | nenhum | Repassado ao `--extractor-args` do yt-dlp |

## Perguntas frequentes

<details>
<summary><b>Quais sites funcionam?</b></summary>

A maioria dos [sites suportados pelo yt-dlp](https://github.com/yt-dlp/yt-dlp/blob/master/supportedsites.md).
Conteúdo com DRM, como o da maioria dos serviços de streaming pagos, não funciona.
</details>

<details>
<summary><b>O YouTube bloqueou o servidor ("confirme que você não é um robô").</b></summary>

O YouTube desconfia de endereços de servidores de nuvem. Tente nesta ordem, reiniciando o serviço depois de cada
mudança em `/etc/videown.env` (`sudo systemctl restart videown`):

1. **Token PO.** Já vem instalado e ligado. Confira se está rodando: `systemctl status videown-pot`. Se o bloqueio
   continuar, ative também `VIDEOWN_EXTRACTOR_ARGS=youtube:player_client=default,mweb`, que faz o yt-dlp usar o
   cliente que mais se beneficia do token.
2. **Só IPv4.** Ative `VIDEOWN_FORCE_IPV4=1`. O YouTube costuma avaliar pior os blocos IPv6 de nuvem.
3. **Cookies de uma conta logada.**
   1. Use uma conta secundária. Contas usadas assim podem ser limitadas.
   2. No seu computador, abra uma janela anônima e entre no YouTube com essa conta. Exporte os cookies em formato
      Netscape (`cookies.txt`), com uma extensão do navegador ou com
      `yt-dlp --cookies-from-browser firefox --cookies cookies.txt`. Depois, feche a janela anônima sem sair da conta.
   3. Copie o arquivo para o servidor e instale:
      `sudo install -o videown -g videown -m 600 cookies.txt /var/lib/videown/cookies.txt`
   4. Ative a linha `VIDEOWN_COOKIES_FILE`. Os cookies vencem de tempos em tempos. Se o bloqueio voltar, repita.
4. **Proxy residencial.** Com `VIDEOWN_PROXY`, o yt-dlp passa a sair por outro endereço, por exemplo uma conexão
   doméstica sua ou um serviço de proxy residencial. É a saída mais confiável, mas o tráfego dos vídeos passa por ela.

Nenhuma dessas soluções é garantida para sempre: o YouTube muda as regras com frequência, e por isso o yt-dlp é
atualizado todos os dias.
</details>

<details>
<summary><b>Como vejo o que está acontecendo?</b></summary>

```bash
journalctl -u videown -f          # registro do serviço
journalctl -u videown-pot -f      # registro do gerador de token
du -sh /var/lib/videown/files     # espaço usado pelos downloads
```
</details>

<details>
<summary><b>Como troco a senha?</b></summary>

Edite `VIDEOWN_PASSWORD` em `/etc/videown.env` e rode `sudo systemctl restart videown`. As sessões abertas são
encerradas.
</details>

<details>
<summary><b>Como desinstalo?</b></summary>

```bash
sudo systemctl disable --now videown videown-pot
sudo rm -rf /opt/videown /var/lib/videown /etc/videown.env
sudo rm -f /etc/systemd/system/videown.service /etc/systemd/system/videown-pot.service
sudo userdel videown
```
</details>

## Segurança

- **Acesso:** sem a senha, nenhuma página ou arquivo abre. Depois de 10 senhas erradas em 15 minutos, o login trava
  por 15 minutos.
- **Links:** só são aceitos links `http(s)` para endereços públicos. Endereços do próprio servidor, da rede interna e
  de metadados da nuvem são recusados.
- **Comandos:** os links nunca passam por um shell e não conseguem virar opções do yt-dlp.
- **Serviço:** roda com um usuário próprio, sem privilégios, isolado pelo systemd, e só escreve na pasta de dados.

Detalhes e limites em [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Desenvolvimento

Requer Rust 1.82 ou mais novo e o `yt-dlp` instalado.

```bash
VIDEOWN_PASSWORD=uma-senha-de-teste cargo run    # http://127.0.0.1:8080
cargo test
```

## Uso responsável

Baixe apenas conteúdo seu ou que você tem permissão para baixar, e respeite os termos de cada site. O Videown não
contorna DRM.
