//! Torna legível um símbolo C++ "mangled", para o relatório mostrar a
//! função exata (`Classe::método`) em vez de `?metodo@Classe@@...`. Cobre o
//! esquema da Microsoft (MSVC, começa com `?`) e o Itanium (começa com `_Z`),
//! os dois que aparecem em binários nativos de Windows, Linux e macOS.
//! Nomes que não são símbolos C++ (as APIs puras de C, como `RegOpenKeyA`)
//! voltam sem mudança.

/// Nome legível do símbolo. Devolve `Classe::método` quando consegue
/// desmanglar; senão, o nome original.
pub fn demangle(name: &str) -> String {
    if name.starts_with('?') {
        // Só o nome qualificado, sem tipo de retorno nem argumentos: é o que
        // serve como inventário de funções.
        let flags = msvc_demangler::DemangleFlags::NAME_ONLY;
        if let Ok(s) = msvc_demangler::demangle(name, flags)
            && !s.is_empty()
        {
            return s;
        }
    }
    if name.starts_with("_Z") || name.starts_with("__Z") {
        let opts = cpp_demangle::DemangleOptions::new().no_params().no_return_type();
        if let Ok(sym) = cpp_demangle::Symbol::new(name)
            && let Ok(s) = sym.demangle_with_options(&opts)
        {
            return s;
        }
    }
    name.to_string()
}

/// Como [`demangle`], mas corta nomes muito longos (templates pesados) para
/// o relatório não estourar.
pub fn demangle_short(name: &str) -> String {
    let d = demangle(name);
    if d.chars().count() > 110 {
        format!("{}…", d.chars().take(110).collect::<String>())
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msvc_vira_classe_metodo() {
        assert_eq!(demangle("?OnSkew@CPaintTool@@QEAAXW4SkewDirection@@H@Z"), "CPaintTool::OnSkew");
    }

    #[test]
    fn api_c_pura_inalterada() {
        assert_eq!(demangle("RegOpenKeyExA"), "RegOpenKeyExA");
        assert_eq!(demangle("CreateWindowExW"), "CreateWindowExW");
    }

    #[test]
    fn itanium_desmangla() {
        // _Z3foov  ->  foo
        assert_eq!(demangle("_Z3foov"), "foo");
    }
}
