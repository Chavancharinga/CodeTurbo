//! O JavaScript que roda dentro da página: achar elementos pelo que a pessoa
//! vê, saber onde clicar e se algo está por cima, listar o que dá para clicar.

use serde_json::Value;

/// Functions every script uses. Elements get a stable `data-ct-ref`, so
/// `@e7` still means the same button in the next step.
const PRELUDE: &str = r#"
const SEL='a[href],button,input,select,textarea,summary,[role=button],[role=link],[role=tab],[role=menuitem],[role=checkbox],[role=switch],[onclick],[tabindex]:not([tabindex="-1"])';
const vis=e=>{const s=getComputedStyle(e),r=e.getBoundingClientRect();return s.visibility!=='hidden'&&s.display!=='none'&&r.width>1&&r.height>1&&parseFloat(s.opacity)>0.05;};
const name=e=>String(e.getAttribute('aria-label')||e.innerText||e.placeholder||e.value||e.alt||e.title||e.name||'').replace(/\s+/g,' ').trim().slice(0,48);
const desc=e=>e?(e.tagName.toLowerCase()+(e.id?'#'+e.id:'')+(typeof e.className==='string'&&e.className.trim()?'.'+e.className.trim().split(/\s+/)[0]:'')):'nada';
window.__ctn=window.__ctn||0;
const refOf=e=>{let id=e.getAttribute('data-ct-ref');if(!id){id='e'+(++window.__ctn);e.setAttribute('data-ct-ref',id);}return id;};
const find=t=>{
  if(t.ref)return document.querySelector('[data-ct-ref="'+t.ref+'"]');
  if(t.selector){try{return document.querySelector(t.selector);}catch(_){return null;}}
  if(t.text){const q=t.text.toLowerCase();const els=[...document.querySelectorAll(SEL)].filter(vis);
    return els.find(e=>name(e).toLowerCase()===q)||els.find(e=>name(e).toLowerCase().includes(q))
      ||[...document.querySelectorAll('body *')].find(e=>vis(e)&&e.children.length===0&&(e.innerText||'').trim().toLowerCase().includes(q))||null;}
  return null;};
"#;

/// A script body (statements that `return`) wrapped to run in the page.
pub fn wrap(body: &str) -> String {
    format!("(()=>{{{PRELUDE}\n{body}\n}})()")
}

fn js(v: &Value) -> String {
    v.to_string()
}

/// Lists what can be clicked or typed into.
pub fn snapshot() -> String {
    r#"const items=[...document.querySelectorAll(SEL)].filter(vis).slice(0,80).map(e=>{const r=e.getBoundingClientRect();
return {ref:refOf(e),tag:e.tagName.toLowerCase(),type:e.type||'',role:e.getAttribute('role')||'',name:name(e),href:e.getAttribute('href')||'',x:Math.round(r.left),y:Math.round(r.top),w:Math.round(r.width),h:Math.round(r.height),disabled:!!e.disabled,checked:!!e.checked};});
return JSON.stringify({url:location.href,title:document.title,items,scrollY:Math.round(scrollY),height:document.documentElement.scrollHeight,vh:innerHeight});"#
        .to_string()
}

/// The snapshot as the lines the agent reads (`@e7 button "Add to cart"`).
pub fn describe_snapshot(v: &Value) -> String {
    let mut out = format!("página {} — \"{}\"", v["url"].as_str().unwrap_or(""), v["title"].as_str().unwrap_or(""));
    let items = v["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        out.push_str("\n(nada clicável nesta tela)");
    }
    for it in &items {
        let kind = match (it["tag"].as_str().unwrap_or(""), it["type"].as_str().unwrap_or(""), it["role"].as_str().unwrap_or("")) {
            ("a", _, _) => "link".to_string(),
            ("input", t, _) if !t.is_empty() && t != "text" => format!("input[{t}]"),
            ("input", _, _) => "input".to_string(),
            (t, _, r) if !r.is_empty() && t != "button" => r.to_string(),
            (t, _, _) => t.to_string(),
        };
        let mut line = format!("\n@{} {kind} \"{}\"", it["ref"].as_str().unwrap_or("?"), it["name"].as_str().unwrap_or(""));
        if let Some(h) = it["href"].as_str().filter(|h| !h.is_empty()) {
            line.push_str(&format!(" → {h}"));
        }
        if it["disabled"] == true {
            line.push_str(" (desabilitado)");
        }
        if it["checked"] == true {
            line.push_str(" (marcado)");
        }
        out.push_str(&line);
    }
    let (y, h, vh) = (v["scrollY"].as_u64().unwrap_or(0), v["height"].as_u64().unwrap_or(0), v["vh"].as_u64().unwrap_or(0));
    if h > vh + 8 {
        out.push_str(&format!("\n(a página tem {h}px de altura; esta tela mostra de {y}px a {}px)", y + vh));
    }
    out
}

/// Brings the element into view and measures where its centre is, and what
/// would really receive a click there.
pub fn prepare(target: &Value, focus: bool) -> String {
    let focus = if focus { "e.focus();" } else { "" };
    format!(
        r#"const T={};const e=find(T);if(!e)return JSON.stringify({{error:'não achei '+JSON.stringify(T)+' na página'}});
e.scrollIntoView({{block:'center',inline:'center'}});{focus}
const r=e.getBoundingClientRect();const x=r.left+r.width/2,y=r.top+r.height/2;const top=document.elementFromPoint(x,y);
const hit=top&&(top===e||e.contains(top)||top.contains(e));
return JSON.stringify({{x,y,name:name(e)||desc(e),tag:desc(e),disabled:!!e.disabled,blocked:hit?'':desc(top),ref:refOf(e),editable:!!e.isContentEditable||/^(INPUT|TEXTAREA)$/.test(e.tagName)}});"#,
        js(target)
    )
}

pub fn select_all(target: &Value) -> String {
    format!("const e=find({});if(e&&e.select)try{{e.select();}}catch(_){{}}return 'ok';", js(target))
}

pub fn value_of(target: &Value) -> String {
    format!("const e=find({});return e?(e.value!==undefined?e.value:e.innerText):'';", js(target))
}

/// The text of the page, or of one element.
pub fn read_text(target: Option<Value>) -> String {
    let pick = match target {
        Some(t) => format!("find({})||document.body", js(&t)),
        None => "document.body".to_string(),
    };
    format!("const e={pick};return (e.innerText||'').replace(/\\n{{3,}}/g,'\\n\\n').trim().slice(0,2500);")
}
