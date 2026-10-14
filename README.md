<p align="center">
  <img src="docs/codeturbo.svg" alt="CodeTurbo: logotipo com um caracol em movimento" width="618">
</p>

<p align="center">
  <a href="https://github.com/Chavancharinga/CodeTurbo/releases/latest"><img alt="Versão" src="https://img.shields.io/github/v/release/Chavancharinga/CodeTurbo?label=vers%C3%A3o&color=2F7BFF"></a>
  <a href="#licença"><img alt="Licença: MIT ou Apache-2.0" src="https://img.shields.io/badge/licen%C3%A7a-MIT%20ou%20Apache--2.0-2F7BFF"></a>
  <img alt="Rust 1.94 ou superior" src="https://img.shields.io/badge/rust-1.94%2B-2F7BFF?logo=rust&logoColor=white">
  <img alt="Escrito integralmente em Rust" src="https://img.shields.io/badge/100%25-Rust-2F7BFF">
</p>

<p align="center"><b>Agente de código para terminal</b> · rápido · econômico · verificado</p>

---

O CodeTurbo é um agente de desenvolvimento de software para o terminal, escrito
integralmente em Rust. Seu objetivo é entregar código correto, compilado e
testado, com o menor consumo de tokens possível em cada tarefa.

## Vantagens

- **Menor custo por tarefa.** O agente consulta e altera somente o trecho de
  código necessário, sem reenviar arquivos inteiros ao modelo, e aproveita o
  cache de prompt dos provedores. Cada interação consome menos tokens e custa
  menos.
- **Código entregue em funcionamento.** Uma tarefa só é considerada concluída
  quando o projeto compila, passa na análise estática sem nenhum alerta e é
  aprovado nos testes. Qualquer falha retorna ao modelo para correção, sem
  intervenção manual.
- **Menos retrabalho.** Cada alteração é analisada no mesmo instante em que é
  feita, para Rust, JavaScript, TypeScript, CSS, HTML, JSON e TOML. Os erros são
  corrigidos antes de se acumularem, o que reduz o número de rodadas e o gasto
  total.
- **Liberdade de provedor.** Funciona com Anthropic, OpenAI, OpenRouter e
  modelos locais via Ollama, que dispensam chave de API e não geram custo de
  uso.
- **Consumo sempre visível.** O raciocínio do modelo, as alterações com
  numeração de linhas, os tokens consumidos, a velocidade de geração, a
  ocupação do contexto, o custo e os limites de uso são exibidos durante todo o
  trabalho.

## Diferenciais

- **Verificação que não pode ser contornada.** Testes removidos ou ignorados e
  alertas silenciados são detectados e recusados. O resultado entregue é o
  resultado real do projeto.
- **Executável único e nativo.** Não depende de Node.js, Python nem de
  servidores externos para funcionar. A análise de código é embutida no próprio
  executável; servidores de linguagem instalados na máquina são aproveitados
  quando disponíveis.
- **Controle sobre cada ação.** Modos de aprovação, regras por ferramenta e por
  comando, e a alteração exibida antes de ser aplicada. O modo plano permite
  revisar e aprovar a estratégia antes de qualquer modificação no projeto.
- **Atualização integrada.** O CodeTurbo avisa quando há uma versão nova e se
  atualiza com um único comando.

## Instalação

No Windows, abra o PowerShell e execute:

```powershell
irm https://raw.githubusercontent.com/Chavancharinga/CodeTurbo/main/install.ps1 | iex
```

No Linux e no macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/Chavancharinga/CodeTurbo/main/install.sh | sh
```

A partir do código-fonte, com Rust 1.94 ou superior:

```sh
cargo install --locked --git https://github.com/Chavancharinga/CodeTurbo coturbo-cli
```

## Uso

Abra um terminal na pasta do seu projeto e execute:

```sh
codeturbo
```

Para usar um modelo local pelo Ollama, sem chave de API:

```sh
codeturbo --provider ollama --model qwen2.5:7b
```

Para usar os provedores em nuvem, defina a chave correspondente na variável de
ambiente `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` ou `OPENROUTER_API_KEY`.

Para executar uma única tarefa e encerrar:

```sh
codeturbo run "adicione validação de e-mail ao formulário de cadastro"
```

As preferências ficam em `%APPDATA%\codeturbo\config.toml` no Windows e em
`~/.config/codeturbo/config.toml` no Linux e no macOS.

## Atualização

O CodeTurbo verifica uma vez por dia se existe uma versão mais recente e
informa ao abrir. Para atualizar:

```sh
codeturbo update
```

Para desativar o aviso, defina `update_check = false` no `config.toml` ou a
variável de ambiente `CODETURBO_NO_UPDATE_CHECK`.

## Situação do projeto

Em desenvolvimento ativo. Próximas etapas: interface de tela cheia com
configuração interna, ampliação dos provedores, medição comparativa do consumo
de tokens e motor de design integrado.

## Licença

Licenciado, à sua escolha, sob uma das seguintes licenças:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- Licença MIT ([LICENSE-MIT](LICENSE-MIT))

Salvo indicação em contrário, qualquer contribuição enviada para inclusão no
projeto fica licenciada da mesma forma, sem termos ou condições adicionais.
