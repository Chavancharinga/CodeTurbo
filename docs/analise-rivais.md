# Análise dos concorrentes e plano de melhorias

Data: 14 de outubro de 2026. Fontes: documentação e repositórios públicos de
cada projeto e análises de terceiros (lista no final).

## Quem lidera

| Agente | Onde roda | O que faz bem | Onde é fraco |
| --- | --- | --- | --- |
| Hermes Agent (Nous Research) | terminal, desktop, mensageiros | memória entre sessões, aprende procedimentos reutilizáveis, mais de 40 ferramentas, subagentes, automações agendadas | como agente de código fica atrás: sem LSP, sem leitura por estrutura (AST), sem IDE |
| Claude Code (Anthropic) | terminal, IDE, web | subagentes com contexto próprio, hooks, checkpoints com voltar atrás, CLAUDE.md, compactação automática, MCP | preso aos modelos da Anthropic; não verifica o resultado sozinho |
| Kilo Code | VS Code, JetBrains, CLI, nuvem | modos Plano, Código, Depuração e Pergunta; orquestrador que divide tarefas entre subagentes; AGENTS.md; 500+ modelos | extensão pesada; depende do editor |
| Cline | VS Code | modos Planejar e Agir, checkpoints a cada passo, Memory Bank em Markdown, compactação automática aos 80% do contexto, barra de contexto | memória feita à mão; gasta muitos tokens em arquivos inteiros |
| OpenClaw | computador do usuário via mensageiros | agente pessoal autônomo, memória, qualquer modelo | não é especializado em código; riscos de segurança |

## O que só o CodeTurbo tem

- **Verificação real obrigatória.** A tarefa só termina depois de compilação,
  análise estática sem alertas e testes aprovados, com detecção de trapaça
  (testes apagados, alertas silenciados). Nenhum dos concorrentes faz isso
  sozinho.
- **Motor de design embutido.** Contrato de tokens, fiscal com mais de 30
  regras e página renderizada no Edge em computador e celular, com sobreposição,
  elementos colados, imagens quebradas e responsividade. Os concorrentes
  dependem só do modelo.
- **Economia por estrutura.** Leitura de arquivos grandes como esqueleto e
  edição por símbolo, sem reenviar arquivos inteiros.
- **Executável único em Rust.** Sem Node.js, sem extensão de editor;
  diagnóstico nativo para Rust, JavaScript, TypeScript, CSS, HTML, JSON e TOML.

## Lacunas a fechar, por prioridade

1. **Memória do projeto (`CODETURBO.md`, compatível com `AGENTS.md` e
   `CLAUDE.md`).** Convenções carregadas em toda sessão. Barato e de alto
   impacto. *Implementado nesta versão.*
2. **Checkpoints e `/desfazer`.** Instantâneo antes de cada edição; voltar a
   qualquer ponto da tarefa. O anti-trapaça já guarda o estado anterior dos
   arquivos; falta expor.
3. **Compactação automática aos 80% do contexto** preservando lista de
   tarefas e decisões, em vez de só ao fim de cada tarefa.
4. **Subagentes de exploração.** Leitura em paralelo com contexto próprio,
   devolvendo só o resumo: menos tokens no contexto principal.
5. **Cliente MCP.** Conecta Mobbin, Figma e o REA externo; todos os
   concorrentes têm.
6. **Capacidade REA nativa.** Investigar como um aplicativo existente faz algo
   (JavaScript/Electron, sites, binários com strings e símbolos, SQLite,
   captura de execução) e usar a evidência para implementar. Começar pelo que
   não exige ferramentas externas: aplicativos JavaScript e Electron, sites com
   captura HAR e metadados de binários.
7. **Esqueleto para TypeScript e JavaScript** com o parser `oxc` já embutido,
   como já existe para Rust: o maior corte de tokens disponível.
8. **Ferramentas somente leitura em paralelo.** Várias leituras e buscas
   executadas ao mesmo tempo: tarefas mais rápidas.
9. **Modo sem interface com saída JSON** para CI e automação.
10. **Aprender procedimentos** (como o Hermes): ao concluir uma tarefa
    complexa e verificada, registrar o passo a passo reutilizável no
    `CODETURBO.md`.

## Visibilidade

O CodeTurbo já se identifica no OpenRouter (cabeçalhos `HTTP-Referer` e
`X-Title`); aparece no ranking de aplicativos quando o uso acumulado for
relevante. Para o ícone do caracol aparecer lá, o endereço de referência
precisa ter o caracol como ícone: uma página no GitHub Pages com o logotipo
resolve.

## Fontes

- [Hermes Agent (Nous Research)](https://agentic-ai.readthedocs.io/en/latest/AgentPlatforms/hermes-agent/)
- [Hermes Agent como agente de código](https://amplifying.ai/coding-agents/hermes-agent)
- [Kilo Code](https://realpython.com/ref/ai-coding-tools/kilo-code/)
- [Kilo Code, código aberto](https://kilocode.ai/landing/open-source)
- [OpenClaw](https://toolpod.dev/blog/openclaw-open-source-ai-agent-guide)
- [Cline: compactação automática](https://docs.cline.bot/features/auto-compact.md)
- [Cline: Memory Bank](https://docs.cline.bot/improving-your-prompting-skills/cline-memory-bank)
- [Claude Code: arquitetura](https://mer.vin/2026/05/claude-code-architecture-explained-six-harness-layers-beyond-the-llm/)
- [Claude Code: hooks](https://mlops.community/blog/the-complete-guide-to-claude-code-hooks-automating-your-ai-coding-workflow)
- [REA](https://github.com/morluto/rea)
