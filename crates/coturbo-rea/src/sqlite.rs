//! SQLite databases: the schema (tables, indexes, views and triggers) from
//! the `sqlite_master` table, read straight from the file format, plus the
//! page size and row estimate. No SQL engine.

fn u16_be(d: &[u8], o: usize) -> usize {
    ((d[o] as usize) << 8) | d[o + 1] as usize
}

fn u32_be(d: &[u8], o: usize) -> u64 {
    ((d[o] as u64) << 24) | ((d[o + 1] as u64) << 16) | ((d[o + 2] as u64) << 8) | d[o + 3] as u64
}

/// A SQLite varint: the value and how many bytes it took.
fn varint(d: &[u8]) -> (i64, usize) {
    let mut v: i64 = 0;
    for i in 0..9 {
        let Some(&b) = d.get(i) else { return (v, i) };
        if i == 8 {
            return ((v << 8) | b as i64, 9);
        }
        v = (v << 7) | (b & 0x7f) as i64;
        if b & 0x80 == 0 {
            return (v, i + 1);
        }
    }
    (v, 9)
}

/// Reads one record's column values (text and ints only — enough for the
/// schema) from a cell body of `page`.
fn record(body: &[u8]) -> Vec<String> {
    let (hdr_len, n) = varint(body);
    let mut types = Vec::new();
    let mut p = n;
    while p < hdr_len as usize && p < body.len() {
        let (t, used) = varint(&body[p..]);
        types.push(t);
        p += used;
    }
    let mut out = Vec::new();
    let mut data = hdr_len as usize;
    for t in types {
        let (text, size) = match t {
            0 => ("NULL".to_string(), 0),
            1 => (if data < body.len() { (body[data] as i8).to_string() } else { "0".into() }, 1),
            2..=6 => ("<int>".to_string(), [0usize, 1, 2, 3, 4, 6, 8][t as usize]),
            7 => ("<real>".to_string(), 8),
            8 => ("0".to_string(), 0),
            9 => ("1".to_string(), 0),
            t if t >= 13 && t % 2 == 1 => {
                let len = ((t - 13) / 2) as usize;
                let s = body.get(data..data + len).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                (s, len)
            }
            t if t >= 12 => ("<blob>".to_string(), ((t - 12) / 2) as usize),
            _ => ("?".to_string(), 0),
        };
        out.push(text);
        data += size;
    }
    out
}

/// Schema rows from page 1 (`sqlite_master`): `(type, name, sql)`.
fn schema(data: &[u8], page_size: usize) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    // Page 1 starts with the 100-byte file header; the b-tree header follows.
    let btree = 100;
    if data.len() < btree + 8 || data[btree] != 0x05 && data[btree] != 0x0d {
        // Only a leaf table page (0x0d) is read directly; interior pages skipped.
    }
    if data.get(btree) != Some(&0x0d) {
        return out; // schema spans multiple pages; the first leaf covers small DBs.
    }
    let cells = u16_be(data, btree + 3);
    let ptr_array = btree + 8;
    for i in 0..cells.min(200) {
        let po = ptr_array + i * 2;
        if po + 1 >= data.len() {
            break;
        }
        let cell = u16_be(data, po);
        if cell == 0 || cell >= page_size {
            continue;
        }
        let (_payload, n1) = varint(&data[cell..]);
        let (_rowid, n2) = varint(&data[cell + n1..]);
        let body = &data[cell + n1 + n2..];
        let cols = record(body);
        if cols.len() >= 5 {
            out.push((cols[0].clone(), cols[1].clone(), cols[4].clone()));
        }
    }
    out
}

/// Report on a SQLite database.
pub fn inspect(data: &[u8]) -> Result<String, String> {
    if data.len() < 100 || &data[..16] != b"SQLite format 3\0" {
        return Err("não é um banco SQLite".to_string());
    }
    let page_size = match u16_be(data, 16) {
        1 => 65536,
        p => p,
    };
    let pages = u32_be(data, 28);
    let mut r = format!("SQLite database: page size {page_size}, {pages} pages ({} KB)\n", data.len() / 1024);
    let rows = schema(data, page_size);
    if rows.is_empty() {
        r.push_str("Schema: não foi possível ler direto (tabela sqlite_master em múltiplas páginas); abra com o REA externo.\n");
        return Ok(r);
    }
    let kinds = ["table", "index", "view", "trigger"];
    for kind in kinds {
        let items: Vec<&(String, String, String)> = rows.iter().filter(|(t, _, _)| t == kind).collect();
        if items.is_empty() {
            continue;
        }
        r.push_str(&format!("{}s ({}):\n", kind, items.len()));
        for (_, name, sql) in items.iter().take(40) {
            let def = sql.replace(['\n', '\r'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
            r.push_str(&format!("  {name}: {}\n", def.chars().take(200).collect::<String>()));
        }
    }
    Ok(r)
}
