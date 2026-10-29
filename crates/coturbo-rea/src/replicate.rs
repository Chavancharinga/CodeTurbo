//! Orientação de replicação fiel. Quando uma investigação revela uma
//! aplicação com interface (um site, um app Electron/JS, um APK ou um
//! binário que usa a GUI nativa do sistema), o relatório passa a exigir que
//! uma recriação reproduza o PADRÃO de tela observado — não um resumo nem
//! uma demonstração simplificada. Isso evita o erro comum de entregar uma
//! grade genérica de blocos quando o alvo é, por exemplo, uma biblioteca de
//! jogos com lista lateral, detalhe em destaque e fila de downloads.

/// Pistas de que o alvo tem interface gráfica e, portanto, um layout a
/// replicar. Para binários nativos olhamos as bibliotecas de GUI do sistema
/// no texto do relatório; sites, apps JS e APKs são sempre interface.
fn has_interface(kind: &str, report: &str) -> bool {
    match kind {
        "site" | "js" => true,
        _ => {
            const GUI: [&str; 8] =
                ["USER32", "GDI32", "COMCTL32", "AppKit", "Cocoa", "gtk", "Qt5", "Qt6"];
            let apk = report.contains("AndroidManifest") || report.contains(".apk");
            apk || GUI.iter().any(|g| report.contains(g))
        }
    }
}

/// O bloco de orientação acrescentado ao relatório quando há interface.
/// Vazio quando o alvo não aparenta ter tela (uma lib de cálculo, um
/// formato de arquivo), onde não há layout a imitar.
pub fn guidance(kind: &str, report: &str) -> String {
    if !has_interface(kind, report) {
        return String::new();
    }
    "\n\nSe o objetivo for RECRIAR esta aplicação: reproduza o PADRÃO de interface \
     observado acima, não uma versão simplificada. Monte o mesmo esqueleto de tela \
     — a navegação ou abas, as listas com busca e agrupamento, o layout mestre-detalhe \
     (lista de um lado, detalhe em destaque do outro), a ação principal com seus estados, \
     as filas e barras de progresso e a barra de status — com a mesma densidade de \
     informação do original. Só depois torne a marca, o código e os visuais próprios. \
     Uma grade de blocos genéricos não é uma réplica; estude a estrutura, não só os rótulos. \
     Os nomes de funções e textos acima não mostram o visual: use observe_app no programa original \
     para ver a tela de verdade (ícones, densidade, controles, cores) e, depois de construir, \
     use observe_app no seu resultado e compare os dois lado a lado até a diferença sumir. \
     Copiar só a tela não basta: desenhe cada ícone como SVG próprio, reproduza as animações e transições \
     (passar o mouse, pressionar, carregar, abrir e fechar) e implemente TODAS as funções do original, com \
     os estados vazio, carregando e erro, atalhos e o que é guardado entre usos. Nada de botão de enfeite. \
     Se o original é um site, use browse nele para ver cada menu, link e formulário funcionando de verdade \
     e depois no seu resultado, passo a passo, até se comportarem igual."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exige_layout_para_interface() {
        assert!(!guidance("site", "qualquer coisa").is_empty());
        assert!(!guidance("js", "rotas e ipc").is_empty());
        // binário com GUI nativa do Windows
        assert!(!guidance("binary", "Imports:\n  USER32.dll: CreateWindowExW").is_empty());
        assert!(!guidance("binary", "contém GDI32 e COMCTL32").is_empty());
    }

    #[test]
    fn silencioso_sem_interface() {
        // um binário de linha de comando sem bibliotecas de GUI
        assert!(guidance("binary", "Imports:\n  KERNEL32.dll: ReadFile").is_empty());
        assert!(guidance("other", "SQLite format 3").is_empty());
    }
}
