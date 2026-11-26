use std::{
    io::{Read, Seek},
    path::Path,
};

use quick_xml::{
    escape::resolve_predefined_entity,
    events::Event,
    name::ResolveResult,
    NsReader, Reader, Writer,
};

use zip::ZipArchive;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn is_supported_office_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("docx" | "xlsx" | "pptx")
    )
}

pub fn detect_office_kind(path: &Path) -> Result<&'static str> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("docx") => Ok("docx"),
        Some("xlsx") => Ok("xlsx"),
        Some("pptx") => Ok("pptx"),
        _ => Err("Unsupported Office file type.".into()),
    }
}

pub fn extract_package_text<R: Read + Seek>(
    kind: &str,
    zip: &mut ZipArchive<R>,
) -> Result<String> {
    match kind {
        "docx" => extract_docx_text(zip),
        "xlsx" => extract_xlsx_text(zip),
        "pptx" => extract_pptx_text(zip),
        _ => Err("Unsupported Office file type.".into()),
    }
}

fn extract_docx_text<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<String> {
    let names = zip.file_names().map(str::to_string).collect::<Vec<_>>();
    extract_docx_parts(names.iter().map(String::as_str), |name| read_zip_text(zip, name))
}

pub fn extract_docx_parts<'a>(
    names: impl IntoIterator<Item = &'a str>,
    mut read_part: impl FnMut(&str) -> Result<String>,
) -> Result<String> {
    let mut names = names
        .into_iter()
        .filter(|name| {
            *name == "word/document.xml"
                || (name.ends_with(".xml")
                    && (name.starts_with("word/header") || name.starts_with("word/footer")))
        })
        .collect::<Vec<_>>();
    names.sort_unstable();
    let mut parts = Vec::new();
    for name in names {
        parts.push(extract_word_xml(&read_part(name)?)?);
    }
    Ok(normalize_text(&parts.join("\n\n")))
}

fn extract_word_xml(xml: &str) -> Result<String> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut output = String::new();
    let mut field_results = Vec::new();
    let mut in_text = false;

    loop {
        let (namespace, event) = reader.read_resolved_event()?;
        let word_element = matches!(namespace, ResolveResult::Bound(ns) if matches!(
            ns.as_ref(),
            b"http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                | b"http://purl.oclc.org/ooxml/wordprocessingml/main"
        ));
        match event {
            Event::Start(element) if word_element => match element.local_name().as_ref() {
                b"fldChar" => {
                    // Field instructions can contain nested fields or EndNote metadata.
                    for attribute in element.attributes() {
                        let attribute = attribute?;
                        if attribute.key.local_name().as_ref() == b"fldCharType" {
                            match attribute.value.as_ref() {
                                b"begin" => field_results.push(false),
                                b"separate" => {
                                    if let Some(result) = field_results.last_mut() {
                                        *result = true;
                                    }
                                }
                                b"end" => {
                                    field_results.pop();
                                }
                                _ => {}
                            }
                            break;
                        }
                    }
                }
                name if field_results.iter().all(|result| *result) => match name {
                    b"t" => in_text = true,
                    b"tab" | b"ptab" => output.push('\t'),
                    b"br" | b"cr" => output.push('\n'),
                    _ => {}
                },
                _ => {}
            },
            Event::End(element) if word_element => match element.local_name().as_ref() {
                b"t" => in_text = false,
                b"p" if field_results.iter().all(|result| *result) => output.push('\n'),
                _ => {}
            },
            Event::Text(text) if in_text => output.push_str(&text.xml10_content()?),
            Event::CData(text) if in_text => output.push_str(&text.xml10_content()?),
            Event::GeneralRef(entity) if in_text => {
                if let Some(character) = entity.resolve_char_ref()? {
                    output.push(character);
                } else if let Some(text) = resolve_predefined_entity(&entity.decode()?) {
                    output.push_str(text);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(output)
}

pub fn equivalent_package_xml(name: &str, previous: &[u8], current: &[u8]) -> bool {
    // OPC relationship and content-type tables are unordered. Re-saving a
    // presentation can reorder these records without changing any references.
    if name.ends_with(".rels") || name == "[Content_Types].xml" {
        return match (package_table(previous), package_table(current)) {
            (Ok(previous), Ok(current)) => previous == current,
            _ => false,
        };
    }

    match (normalize_package_xml(previous, name), normalize_package_xml(current, name)) {
        (Ok(previous), Ok(current)) => previous == current,
        _ => false,
    }
}

fn normalize_package_xml(bytes: &[u8], name: &str) -> Result<Vec<u8>> {
    let word_part = name.starts_with("word/");
    let worksheet = name.starts_with("xl/worksheets/") && name.ends_with(".xml");
    let powerpoint = name.starts_with("ppt/");
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().expand_empty_elements = true;
    let mut writer = Writer::new(Vec::new());
    let mut skipped = 0;
    let mut depth = 0;
    let mut generated_field = None;
    loop {
        let event = reader.read_event()?;
        if skipped > 0 {
            match event {
                Event::Start(_) => skipped += 1,
                Event::End(_) => skipped -= 1,
                Event::Eof => return Err("Unexpected end of Office XML.".into()),
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(ref element) => {
                let local = element.local_name();
                if (word_part && matches!(local.as_ref(), b"rsids" | b"proofErr" | b"lastRenderedPageBreak"))
                    || (worksheet && local.as_ref() == b"selection")
                {
                    skipped = 1;
                    continue;
                }
                let mut attributes = element.attributes().collect::<std::result::Result<Vec<_>, _>>()?;
                // MS-PPTX §2.2.11 defines these extensions as editing guides.
                if powerpoint && element.name().as_ref() == b"p:ext"
                    && attributes.iter().any(|attribute| {
                        attribute.key.as_ref() == b"uri" && is_powerpoint_guide(attribute.value.as_ref())
                    })
                {
                    skipped = 1;
                    continue;
                }
                if powerpoint && generated_field.is_some() && element.name().as_ref() == b"a:t" {
                    skipped = 1;
                    continue;
                }
                if powerpoint && element.name().as_ref() == b"a:fld" && attributes.iter().any(|attribute| {
                    attribute.key.as_ref() == b"type" && (
                    attribute.value.starts_with(b"datetime") || attribute.value.as_ref() == b"slidenum"
                )
                }) {
                    generated_field = Some(depth);
                }
                depth += 1;
                attributes.retain(|attribute| {
                    let local = attribute.key.local_name();
                    let name = local.as_ref();
                    !word_part || !(name.starts_with(b"rsid") || matches!(name, b"paraId" | b"textId"))
                });
                if powerpoint {
                    let element = element.name();
                    // PowerPoint updates these identifiers on editing. Keep
                    // the element itself and all actual presentation properties.
                    attributes.retain(|attribute| {
                        !(matches!(
                            (element.as_ref(), attribute.key.as_ref()),
                            (b"p14:modId" | b"p14:creationId", b"val")
                                | (b"p:presentation", b"bookmarkIdSeed")
                        ) || (name == "ppt/presentation.xml"
                            && element.as_ref() == b"p:sldId"
                            && attribute.key.as_ref() == b"id"))
                    });
                }
                attributes.sort_unstable_by(|a, b| a.key.as_ref().cmp(b.key.as_ref()));
                let mut normalized = element.clone().into_owned();
                normalized.clear_attributes();
                for attribute in attributes {
                    normalized.push_attribute(attribute);
                }
                writer.write_event(Event::Start(normalized))?;
            }
            Event::End(_) => {
                depth -= 1;
                if generated_field == Some(depth) {
                    generated_field = None;
                }
                writer.write_event(event)?;
            }
            Event::Decl(_) | Event::Comment(_) => {}
            Event::Eof => break,
            event => writer.write_event(event)?,
        }
    }
    Ok(writer.into_inner())
}

fn package_table(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().expand_empty_elements = true;
    let mut depth = 0;
    let mut rows = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(element) if depth < 2 => {
                let mut attributes = element.attributes().collect::<std::result::Result<Vec<_>, _>>()?;
                attributes.sort_unstable_by(|a, b| a.key.as_ref().cmp(b.key.as_ref()));
                let mut row = element.clone().into_owned();
                row.clear_attributes();
                for attribute in attributes {
                    row.push_attribute(attribute);
                }
                rows.push(row.to_vec());
                depth += 1;
            }
            Event::End(_) if depth > 0 => depth -= 1,
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Comment(_) | Event::Decl(_) => {}
            Event::Eof if depth == 0 && !rows.is_empty() => break,
            _ => return Err("Unexpected structure in Office package table.".into()),
        }
    }
    rows[1..].sort_unstable();
    Ok(rows)
}

fn is_powerpoint_guide(uri: &[u8]) -> bool {
    [
        "{EFAFB233-063F-42B5-8137-9DF3F51BA10A}",
        "{2D200454-40CA-4A62-9FC3-DE9A4176ACB9}",
        "{DCECCB84-F9BA-43D5-87BE-67443E8EF086}",
        "{27BBF7A9-308A-43DC-89C8-2F10F3537804}",
        "{56416CCD-93CA-4268-BC5B-53C4BB910035}",
        "{620B2872-D7B9-4A21-9093-7833F8D536E1}",
    ].iter().any(|known| uri.eq_ignore_ascii_case(known.as_bytes()))
}

fn extract_xlsx_text<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<String> {
    let shared_strings = read_zip_text(zip, "xl/sharedStrings.xml")
        .map(|xml| {
            extract_xml_text(&xml)
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut parts = Vec::new();
    for name in sorted_names(zip) {
        if name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml") {
            let xml = read_zip_text(zip, &name)?;
            let mut text = extract_xml_text(&xml);
            for (index, value) in shared_strings.iter().enumerate() {
                text = text.replace(&format!("\n{index}\n"), &format!("\n{value}\n"));
            }
            parts.push(format!("# {name}\n{text}"));
        }
    }
    Ok(normalize_text(&parts.join("\n\n")))
}

fn extract_pptx_text<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<String> {
    let names = sorted_names(zip);
    extract_pptx_parts(names.iter().map(String::as_str), |name| read_zip_text(zip, name))
}

pub fn extract_pptx_parts<'a>(
    names: impl IntoIterator<Item = &'a str>,
    mut read_part: impl FnMut(&str) -> Result<String>,
) -> Result<String> {
    let mut names = names.into_iter()
        .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        .collect::<Vec<_>>();
    names.sort_unstable();
    let mut parts = Vec::new();
    for name in names {
        let xml = read_part(name)?;
        let normalized = normalize_package_xml(xml.as_bytes(), name)?;
        parts.push(format!("# {name}\n{}", extract_xml_text(std::str::from_utf8(&normalized)?)));
    }
    Ok(normalize_text(&parts.join("\n\n")))
}

fn sorted_names<R: Read + Seek>(zip: &ZipArchive<R>) -> Vec<String> {
    let mut names = zip.file_names().map(str::to_string).collect::<Vec<_>>();
    names.sort_by(|a, b| natord(a, b));
    names
}

fn read_zip_text<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str) -> Result<String> {
    let mut file = zip.by_name(name)?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(text)
}

fn extract_xml_text(xml: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    let mut entity = String::new();
    let mut in_entity = false;

    for ch in xml.chars() {
        match ch {
            '<' => {
                flush_word_gap(&mut output);
                in_tag = true;
                in_entity = false;
                entity.clear();
            }
            '>' => in_tag = false,
            '&' if !in_tag => {
                in_entity = true;
                entity.clear();
            }
            ';' if in_entity => {
                output.push_str(decode_entity(&entity));
                in_entity = false;
                entity.clear();
            }
            _ if in_entity => entity.push(ch),
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }

    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_entity(entity: &str) -> &str {
    match entity {
        "lt" => "<",
        "gt" => ">",
        "amp" => "&",
        "quot" => "\"",
        "apos" => "'",
        _ => "",
    }
}

fn flush_word_gap(output: &mut String) {
    if !output.ends_with('\n') {
        output.push('\n');
    }
}

fn normalize_text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn natord(a: &str, b: &str) -> std::cmp::Ordering {
    a.cmp(b)
}
