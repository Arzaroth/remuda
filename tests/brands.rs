//! The page draws each provider with the mark and colour TokenGauge draws it
//! with, which both take from `selvedge::brands`. `web/src/icons.ts` is that
//! table written out for the page; `REMUDA_BLESS=1 cargo test --test brands`
//! rewrites it after selvedge moves.

const ICONS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/web/src/icons.ts");

fn view_box(svg: &str) -> &str {
    let at = svg.find("viewBox=\"").expect("a mark has a viewBox") + "viewBox=\"".len();
    &svg[at..at + svg[at..].find('"').unwrap()]
}

const ROOT_ONLY: &[&str] = &[
    "width",
    "height",
    "viewBox",
    "xmlns",
    "xmlns:xlink",
    "version",
];

fn inner(svg: &str) -> String {
    let open = svg.find("<svg").expect("a mark is an svg");
    let tag_end = open + svg[open..].find('>').unwrap();
    let body: String = svg[tag_end + 1..svg.rfind("</svg>").unwrap()]
        .lines()
        .map(str::trim)
        .collect();
    let kept: Vec<&str> = attributes(&svg[open + 4..tag_end])
        .into_iter()
        .filter(|a| !ROOT_ONLY.contains(&a.split('=').next().unwrap()))
        .collect();
    if kept.is_empty() {
        body
    } else {
        format!("<g {}>{body}</g>", kept.join(" "))
    }
}

fn attributes(tag: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = tag.trim();
    while let Some(eq) = rest.find("=\"") {
        let close = eq + 2 + rest[eq + 2..].find('"').unwrap();
        out.push(&rest[..=close]);
        rest = rest[close + 1..].trim_start();
    }
    out
}

fn expected() -> String {
    let mut ts = String::from(
        "// Written by tests/brands.rs from selvedge::brands.\n\
         export const ICONS: Record<string, { viewBox: string; inner: string; color: string | null; mark: string | null }> = {\n",
    );
    for b in selvedge::brands::BRANDS {
        ts += &format!(
            "  {}: {{ viewBox: {}, inner: {}, color: {}, mark: {} }},\n",
            b.id,
            serde_json::to_string(view_box(b.svg)).unwrap(),
            serde_json::to_string(&inner(b.svg)).unwrap(),
            serde_json::to_string(&b.color).unwrap(),
            serde_json::to_string(&b.mark_color()).unwrap(),
        );
    }
    ts + "};\n"
}

#[test]
fn the_page_draws_every_provider_as_selvedge_does() {
    let want = expected();
    if std::env::var_os("REMUDA_BLESS").is_some() {
        std::fs::write(ICONS, &want).unwrap();
    }
    let have = std::fs::read_to_string(ICONS).unwrap();
    assert!(
        have == want,
        "web/src/icons.ts is not selvedge's marks; run REMUDA_BLESS=1 cargo test --test brands, then pnpm build in web/"
    );
}
