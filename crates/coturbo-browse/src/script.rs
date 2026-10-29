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
    let mut out = format!(
        "page {} — \"{}\"",
        v["url"].as_str().unwrap_or(""),
        v["title"].as_str().unwrap_or("")
    );
    let items = v["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        out.push_str("\n(nothing clickable on this screen)");
    }
    for it in &items {
        let kind = match (
            it["tag"].as_str().unwrap_or(""),
            it["type"].as_str().unwrap_or(""),
            it["role"].as_str().unwrap_or(""),
        ) {
            ("a", _, _) => "link".to_string(),
            ("input", t, _) if !t.is_empty() && t != "text" => format!("input[{t}]"),
            ("input", _, _) => "input".to_string(),
            (t, _, r) if !r.is_empty() && t != "button" => r.to_string(),
            (t, _, _) => t.to_string(),
        };
        let mut line = format!(
            "\n@{} {kind} \"{}\"",
            it["ref"].as_str().unwrap_or("?"),
            it["name"].as_str().unwrap_or("")
        );
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
    let (y, h, vh) = (
        v["scrollY"].as_u64().unwrap_or(0),
        v["height"].as_u64().unwrap_or(0),
        v["vh"].as_u64().unwrap_or(0),
    );
    if h > vh + 8 {
        out.push_str(&format!(
            "\n(the page is {h}px tall; this screen shows {y}px to {}px)",
            y + vh
        ));
    }
    out
}

/// Brings the element into view and measures where its centre is, and what
/// would really receive a click there.
pub fn prepare(target: &Value, focus: bool) -> String {
    let focus = if focus { "e.focus();" } else { "" };
    format!(
        r#"const T={};const e=find(T);if(!e)return JSON.stringify({{error:'could not find '+JSON.stringify(T)+' on the page'}});
e.scrollIntoView({{block:'center',inline:'center'}});{focus}
const r=e.getBoundingClientRect();const x=r.left+r.width/2,y=r.top+r.height/2;const top=document.elementFromPoint(x,y);
const hit=top&&(top===e||e.contains(top)||top.contains(e));
return JSON.stringify({{x,y,name:name(e)||desc(e),tag:desc(e),disabled:!!e.disabled,blocked:hit?'':desc(top),ref:refOf(e),editable:!!e.isContentEditable||/^(INPUT|TEXTAREA)$/.test(e.tagName)}});"#,
        js(target)
    )
}

pub fn select_all(target: &Value) -> String {
    format!(
        "const e=find({});if(e&&e.select)try{{e.select();}}catch(_){{}}return 'ok';",
        js(target)
    )
}

pub fn value_of(target: &Value) -> String {
    format!(
        "const e=find({});return e?(e.value!==undefined?e.value:e.innerText):'';",
        js(target)
    )
}

/// The text of the page, or of one element.
pub fn read_text(target: Option<Value>) -> String {
    let pick = match target {
        Some(t) => format!("find({})||document.body", js(&t)),
        None => "document.body".to_string(),
    };
    format!(
        "const e={pick};return (e.innerText||'').replace(/\\n{{3,}}/g,'\\n\\n').trim().slice(0,2500);"
    )
}

/// The visible texts of the page with where they are, so a small picture of
/// it can still be read: headings, paragraphs, links, buttons, fields.
pub fn labels() -> String {
    r#"const out=[];const W=innerWidth,H=innerHeight;
const els=document.querySelectorAll('h1,h2,h3,h4,h5,p,li,a,button,label,summary,th,td,dt,dd,input,textarea,select,figcaption,span,strong,em,[role=button],[role=tab],[role=menuitem]');
for(const e of els){
  const tag=e.tagName.toLowerCase();
  let t='';
  if(tag==='input'||tag==='textarea'){t=e.value||e.placeholder||'';}
  else if(tag==='select'){t=(e.selectedOptions&&e.selectedOptions[0]&&e.selectedOptions[0].textContent)||'';}
  else{t=[...e.childNodes].filter(n=>n.nodeType===3).map(n=>n.textContent).join(' ');}
  t=t.replace(/\s+/g,' ').trim();
  if(!t||!vis(e))continue;
  const r=e.getBoundingClientRect();
  if(r.bottom<0||r.top>H||r.right<0||r.left>W)continue;
  out.push({x:Math.round(r.left),y:Math.round(r.top),w:Math.round(r.width),h:Math.round(r.height),t:t.slice(0,90),k:tag});
  if(out.length>=160)break;
}
return JSON.stringify(out);"#
        .to_string()
}

/// Draws the agent's pointer and its clicks on the page itself, so a person
/// watching the real window sees where it goes: an arrow that glides to the
/// target and a ring that opens where it clicks. It ignores the mouse, so it
/// never gets in the way of what it is pointing at.
pub const POINTER: &str = r##"(()=>{
if(window.__ctCursor)return;
const c=document.createElement('div');c.id='__ct_cursor';
c.style.cssText='position:fixed;left:0;top:0;width:30px;height:30px;z-index:2147483647;pointer-events:none;transform:translate(-120px,-120px);transition:transform 380ms cubic-bezier(.25,.8,.3,1);filter:drop-shadow(0 2px 3px rgba(0,0,0,.45))';
c.innerHTML='<svg width="30" height="30" viewBox="0 0 30 30"><path d="M4 2 L4 23 L9.6 18.2 L13.4 26.4 L16.6 25 L12.9 17 L20.4 16.6 Z" fill="#fff" stroke="#111" stroke-width="1.7" stroke-linejoin="round"/></svg>';
(document.documentElement||document.body).appendChild(c);window.__ctCursor=c;
window.__ctMove=(x,y)=>{c.style.transform='translate('+x+'px,'+y+'px)';};
window.__ctClick=(x,y)=>{const r=document.createElement('div');
r.style.cssText='position:fixed;left:'+(x-19)+'px;top:'+(y-19)+'px;width:38px;height:38px;box-sizing:border-box;border-radius:50%;border:3px solid #ffd650;background:rgba(255,214,80,.25);z-index:2147483646;pointer-events:none;opacity:.95;transform:scale(.5);transition:transform 650ms cubic-bezier(.2,.7,.3,1),opacity 650ms ease-out';
(document.documentElement||document.body).appendChild(r);
requestAnimationFrame(()=>requestAnimationFrame(()=>{r.style.transform='scale(2.3)';r.style.opacity='0';}));
setTimeout(()=>r.remove(),800);};
})();"##;
