# Arquitetura

O Videown é um único processo em Rust, só com a biblioteca padrão. A página web fica embutida no binário. Os
downloads são feitos pelo yt-dlp, que o Videown chama como processo filho.

```
navegador ──HTTPS──▶ proxy reverso ──HTTP──▶ videown (127.0.0.1:8080) ──▶ yt-dlp ──▶ ffmpeg / deno
                                                │
                                                └── /var/lib/videown
                                                      ├── files/<id>/   arquivos baixados
                                                      ├── jobs/<id>.meta  estado de cada pedido
                                                      ├── cache/          cache do yt-dlp
                                                      └── bin/            yt-dlp e deno
```

## Código

| Arquivo | Função |
|---|---|
| `src/main.rs` | Lê a configuração das variáveis de ambiente e inicia tudo |
| `src/http.rs` | Servidor HTTP/1.1 mínimo: uma thread por conexão, limites de tamanho e tempo |
| `src/auth.rs` | Senha, sessões e limite de tentativas |
| `src/routes.rs` | Páginas e API |
| `src/jobs.rs` | Fila, execução do yt-dlp, progresso, limpeza e atualização |
| `src/storage.rs` | Onde os arquivos prontos ficam |
| `web/` | HTML, CSS e JavaScript da página |

## Fluxo de um download

1. A página envia `POST /api/jobs` com o link e o formato.
2. O link é validado e o pedido entra na fila. O estado é salvo em `jobs/<id>.meta`.
3. Uma das threads de download, que são `VIDEOWN_MAX_PARALLEL` no total, pega o pedido e roda o yt-dlp num grupo de
   processos próprio. O Videown acompanha o progresso pelas linhas que o yt-dlp imprime, com `--progress-template` e
   `--print`.
4. No fim, `Storage::finalize` recebe o arquivo, e o pedido fica pronto.
5. A página consulta `GET /api/jobs` a cada segundo enquanto há downloads ativos, e com menos frequência quando não
   há. O arquivo é entregue por `GET /files/<id>`. No Linux, ele vai do disco para a rede por `sendfile`, sem passar
   pela memória do programa.
6. Uma thread de limpeza roda a cada 10 minutos. Ela apaga os pedidos cujo prazo venceu e as pastas sem pedido.

Cancelar mata o grupo de processos inteiro, ou seja, o yt-dlp e o FFmpeg que ele abriu. Se o servidor reiniciar, os
pedidos que estavam na fila ou baixando voltam para a fila.

## API

Toda rota, exceto `/login`, `/style.css`, `/icon.svg` e `/healthz`, exige o cookie de sessão. Todo `POST` exige o
cabeçalho `X-Requested-With: videown`. Os corpos são `application/x-www-form-urlencoded`.

| Método e rota | Corpo | Resposta |
|---|---|---|
| `POST /login` | `password` | Redireciona e define o cookie de sessão |
| `POST /logout` | | `{}` |
| `GET /api/jobs` | | `{jobs, retention_hours, ytdlp, now}` |
| `POST /api/jobs` | `url`, `format` (`best`, `1080`, `720`, `480`, `mp3`, `m4a`) | `{id}` |
| `POST /api/jobs/<id>/cancel` | | `{}` |
| `POST /api/jobs/<id>/delete` | | `{}` |
| `GET /files/<id>` | | O arquivo |

## Armazenamento

`Storage` separa onde os arquivos ficam do resto do programa. A implementação atual, `LocalDisk`, usa uma pasta
local. Outra implementação, para S3 ou um serviço de nuvem, poderia:

- enviar o arquivo em `finalize`;
- devolver `Delivery::Redirect` com um link temporário em `deliver`;
- apagar o arquivo remoto em `delete`.

## Segurança e limites conhecidos

- **Senha:** fica em texto puro em `/etc/videown.env`, que só o root consegue ler. A comparação é feita em tempo
  constante.
- **Sessões:** ficam na memória e valem 30 dias. Reiniciar o serviço encerra todas.
- **Proteção contra outros sites:** o cookie é `SameSite=Strict`, e todo `POST` exige um cabeçalho próprio da
  página.
- **Validação de links:** olha o texto do link. Ela recusa endereços numéricos privados, `localhost` e nomes
  internos, mas não resolve o DNS. Um domínio público que aponte para um IP interno não é detectado.
- **Limites do HTTP:** até 16 KB de cabeçalho, 64 KB de corpo, 64 conexões simultâneas e 15 s para receber uma
  requisição.
- **yt-dlp:** é chamado com `--ignore-config`, o link vem depois de `--` e não há campo de argumentos livres.
- **systemd:** o serviço usa `ProtectSystem=strict`, `NoNewPrivileges` e nenhuma capability, e só escreve em
  `/var/lib/videown`.
