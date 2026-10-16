//! O caminho da pasta do projeto, do jeito que cabe na tela.

/// Prefixo "verbatim" que o Windows põe nos caminhos canonicalizados.
fn plain(path: &str) -> &str {
    path.strip_prefix(r"\\?\").unwrap_or(path)
}

/// O caminho como texto, sem o prefixo verbatim do Windows.
pub fn folder_text(path: &std::path::Path) -> String {
    plain(&path.display().to_string()).to_string()
}

/// `path` em no máximo `max` colunas. Cabendo inteiro, aparece inteiro;
/// senão, só as 3 últimas pastas (`…\Music\CoTurbon`), e se nem assim couber,
/// as 2 últimas, a última, ou o fim do nome com reticências.
pub fn short_path(path: &str, max: usize) -> String {
    let path = plain(path);
    let width = |s: &str| s.chars().count();
    if width(path) <= max {
        return path.to_string();
    }
    let sep = if path.contains('\\') { '\\' } else { '/' };
    let parts: Vec<&str> = path.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    for n in [3, 2, 1] {
        if parts.len() < n {
            continue;
        }
        let tail = parts[parts.len() - n..].join(&sep.to_string());
        let shown = format!("…{sep}{tail}");
        if width(&shown) <= max {
            return shown;
        }
    }
    let last = parts.last().copied().unwrap_or(path);
    let keep = max.saturating_sub(1);
    let tail: String = last.chars().rev().take(keep).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cabendo_aparece_inteiro() {
        assert_eq!(short_path(r"C:\Users\danie\Music\CoTurbon", 60), r"C:\Users\danie\Music\CoTurbon");
    }

    #[test]
    fn grande_demais_fica_com_as_tres_ultimas_pastas() {
        assert_eq!(short_path(r"C:\Users\danie\Music\CoTurbon\sandbox\paint", 30), r"…\CoTurbon\sandbox\paint");
        assert_eq!(short_path("/home/danie/projetos/loja/frontend/src", 28), "…/loja/frontend/src");
    }

    #[test]
    fn se_nem_tres_cabem_usa_menos() {
        assert_eq!(short_path(r"C:\Users\danie\Music\CoTurbon\sandbox\paint", 15), r"…\sandbox\paint");
        assert_eq!(short_path(r"C:\Users\danie\Music\CoTurbon\sandbox\paint", 8), "…\\paint");
    }

    #[test]
    fn nome_unico_enorme_e_cortado_pelo_inicio() {
        let s = short_path(r"C:\a\uma_pasta_com_um_nome_muito_comprido", 12);
        assert_eq!(s.chars().count(), 12);
        assert!(s.starts_with('…') && s.ends_with("comprido"), "{s}");
    }

    #[test]
    fn o_prefixo_verbatim_do_windows_sai() {
        assert_eq!(short_path(r"\\?\C:\Users\danie\CoTurbon", 60), r"C:\Users\danie\CoTurbon");
    }

    #[test]
    fn nunca_passa_do_limite() {
        let p = r"C:\Users\danie\Music\CoTurbon\sandbox\paint\src";
        for max in 1..60 {
            assert!(short_path(p, max).chars().count() <= max.max(1), "max {max}: {}", short_path(p, max));
        }
    }
}
