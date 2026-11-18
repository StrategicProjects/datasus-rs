# datasus-rs — notas para o Claude Code

Leitor de arquivos `.dbc`/`.dbf` do DATASUS em Rust, compartilhado por três front ends. Repositório
`StrategicProjects/datasus-rs`; 0.1.0 publicada em 2026-11-18 (crates.io, PyPI, Homebrew, Zenodo).

- `crates/datasus-core` — port do leitor C do pacote R `datasusr` (`~/Github/datasusr/src`): `blast.rs`
  (PKWare DCL), `container.rs` (detecção DBC/DBF), `dbf.rs` (tipos, parser, trait `Sink`). **Sem
  dependências, e deve continuar assim**: o pacote R vendoriza este crate para o CRAN.
- `crates/datasus-arrow` — core → lotes Arrow. `crates/datasus-cli` — binário `datasus`.
- `crates/datasus-py` — pacote `datasuspy` (PyO3 + maturin, abi3 ≥ 3.10), fora dos `default-members`:
  compilar com `maturin develop --release` num venv, não com `cargo build`.
- Escopo: só o leitor. Catálogo, FTP e cache ficam no `datasusr`.

## Validação

```sh
cargo clippy --workspace --all-targets -- -D warnings && cargo test
cargo build --release -p datasus-cli
Rscript tests/oracle/compare.R <binário datasus> ~/Github/datasusr/tests/testthat/helper-dbf.R [arquivos .dbc]
cd crates/datasus-py && maturin develop --release && pytest tests
```

- O oráculo compara a CLI com o `datasusr` instalado (140/140 idênticas em 2026-11-18). Qualquer mudança
  de comportamento do leitor precisa continuar batendo, ou virar mudança também no `datasusr`.
- `~/.cargo/config.toml` manda o build para `~/.cargo/shared-target`, que outras sessões apagam em
  limpezas de disco: o binário fica em `~/.cargo/shared-target/release/datasus` e pode sumir — recompile.
- O oráculo roda com `LC_CTYPE=C` e `iconv` explícito de propósito: o leitor C usa `isspace()` (no macOS
  com locale UTF-8 remove também `0xA0`) e o R no macOS converte `latin1` como CP1252. O core usa só os
  seis espaços ASCII e ISO-8859-1 estrito. Diferenças conhecidas estão no README.

## Release

1. Subir `version` em `[workspace.package]` do `Cargo.toml` (vale para todos os crates e para o `datasuspy`).
2. Commit, push, CI verde. `git tag -a vX.Y.Z` + push: o workflow `python-release.yml` compila as wheels,
   testa e publica no PyPI por trusted publishing (environment `pypi`, sem token).
3. `gh release create vX.Y.Z --verify-tag` — é a release que faz o Zenodo arquivar (DOI geral
   10.5281/zenodo.24329435).
4. `cargo publish -p datasus-core`, depois `-p datasus-arrow`, depois `-p datasus-cli` (token em
   `~/.cargo/credentials.toml`).
5. Homebrew: atualizar `url` e `sha256` em `packaging/homebrew/datasus.rb`, copiar para
   `$(brew --repository strategicprojects/tap)/Formula/` e dar push no tap. Testar com
   `brew install --build-from-source` instala o `rust` do Homebrew, que passa na frente do cargo do rustup
   no PATH: rodar `brew uninstall rust` depois.

Uma versão publicada não pode ser reutilizada em nenhum desses registros.

## Site

- `site/` é a página estática (um `index.html` + `assets/how-it-works.svg`, sem build), nos moldes de
  `~/Github/foresight/site`. URL: https://strategicprojects.github.io/datasus-rs/.
- O workflow `pages.yml` roda a cada push no `main` que toque `site/`, o core ou o arrow: gera a referência
  com `cargo doc --no-deps -p datasus-core -p datasus-arrow` (com `RUSTDOCFLAGS=-D warnings`) em
  `site/api` (ignorado pelo git) e publica. O Pages do repositório precisa estar com source = GitHub Actions.
- Os números da seção "Speed" e a saída do terminal no topo foram medidos em 2026-11-17/18; ao mudar o
  leitor ou a CLI, conferir se ainda valem. A tabela de diferenças repete a do README: alterar as duas.
- Conferir localmente: `cargo doc --no-deps -p datasus-core -p datasus-arrow`, copiar o `doc` do diretório
  de build (`~/.cargo/shared-target/doc`) para `site/api` e `python3 -m http.server -d site`.

## Decisões

- `blast()` tem limite de saída: a área de registros de um DBC só é descomprimida até o tamanho declarado
  no cabeçalho, e nada passa de 4 GiB (`decode_with_limit` muda o teto). Não remover.
- Colunas de texto nunca são nulas (campo em branco = string vazia); um valor termina no primeiro NUL.
- `i32::MIN` não é aceito como inteiro (é o `NA` do R).
- Migração do `datasusr` para este core: experimento em `~/Github/datasusr-rust` (branch `rust-core`);
  decisão em aberto — ver o `CLAUDE.md` do `datasusr`.
