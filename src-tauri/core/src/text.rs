//! Разбор вывода терминала: склейка потока, управляющие последовательности, поиск адреса сервера.

/// Склейка потока UTF-8, который приходит кусками: граница куска может разрезать символ пополам.
/// Хвост неполного символа придерживается до следующего куска.
#[derive(Default)]
pub struct Utf8Stream {
    rest: Vec<u8>,
}

impl Utf8Stream {
    pub fn feed(&mut self, bytes: &[u8]) -> String {
        self.rest.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&self.rest) {
            Ok(_) => self.rest.len(),
            // Символ оборван концом куска: отдаём всё до него.
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            // Настоящий мусор посреди потока: заменяем его и не застреваем.
            Err(_) => {
                let text = String::from_utf8_lossy(&self.rest).into_owned();
                self.rest.clear();
                return text;
            }
        };
        let tail = self.rest.split_off(valid);
        String::from_utf8(std::mem::replace(&mut self.rest, tail)).unwrap_or_default()
    }
}

/// Строка терминала как обычный текст: без раскраски и без перерисовок.
/// Индикаторы прогресса переписывают строку через `\r`, поэтому берётся её последняя версия.
pub fn plain_line(raw: &str) -> String {
    let plain = strip_ansi(raw);
    let line = plain.trim_end_matches(['\r', '\n']);
    line.rsplit('\r').next().unwrap_or("").trim().to_string()
}

/// Убирает управляющие последовательности терминала. Vite красит адрес по частям
/// (`http://localhost:` одним цветом, порт жирным), и без этого адрес в выводе не найти.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: параметры, затем завершающий символ из диапазона @..~
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: до BEL или ESC \
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Обрезает строку до `max` символов, не разрывая символ посередине.
pub fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// Слова, по которым строка вывода похожа на объяснение, почему процесс завершился.
const REASON_WORDS: [&str; 16] = [
    "eaddrinuse", "in use", "занят", "error", "ошибк", "failed", "не удалось", "cannot", "can't",
    "not found", "не найден", "exception", "fatal", "denied", "отказано", "missing",
];

/// Пояснение к завершению процесса из хвоста его вывода: последняя строка, похожая на причину,
/// а если таких нет — просто последняя. Причина часто стоит не в самом конце: за «порт занят»
/// обычно следует подсказка, что с этим делать.
pub fn pick_reason<'a>(tail: impl IntoIterator<Item = &'a String>) -> Option<String> {
    let lines: Vec<&String> = tail.into_iter().collect();
    lines
        .iter()
        .rev()
        .find(|line| {
            let lower = line.to_lowercase();
            REASON_WORDS.iter().any(|word| lower.contains(word))
        })
        .or(lines.last())
        .map(|line| line.to_string())
}

/// Сообщение об ошибке для показа пользователю начинается с заглавной буквы.
pub fn capitalize(text: String) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => text,
    }
}

const LOCAL_HOSTS: [(&str, &str); 5] = [
    ("localhost", "localhost"),
    ("127.0.0.1", "127.0.0.1"),
    ("[::1]", "[::1]"),
    // «Слушаю на всех интерфейсах» в браузере не открыть: подменяем на localhost.
    ("0.0.0.0", "localhost"),
    ("[::]", "localhost"),
];

/// Первый локальный адрес в строке вывода: `http://localhost:5173/`.
/// Адреса в локальной сети пропускаем: открывать нужно то, что точно относится к этой машине.
pub fn find_local_url(line: &str) -> Option<String> {
    let plain = strip_ansi(line);
    let mut from = 0;
    while let Some(pos) = plain[from..].find("http") {
        let at = from + pos;
        from = at + 4;
        let rest = &plain[at..];
        let (scheme, after) = if let Some(r) = rest.strip_prefix("http://") {
            ("http://", r)
        } else if let Some(r) = rest.strip_prefix("https://") {
            ("https://", r)
        } else {
            continue;
        };
        for (host, shown) in LOCAL_HOSTS {
            let Some(tail) = after.strip_prefix(host) else { continue };
            // После хоста допустимы только порт, путь или конец адреса: `localhost.example.com` не наш.
            if !matches!(tail.chars().next(), None | Some(':' | '/' | ' ' | '\t' | '"' | '\'' | ')')) {
                continue;
            }
            let end = tail
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | '(' | '`' | ','))
                .unwrap_or(tail.len());
            let tail = tail[..end].trim_end_matches(['.', ';', ':']);
            return Some(format!("{scheme}{shown}{tail}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_counts_characters_not_bytes() {
        assert_eq!(shorten("порт занят", 20), "порт занят");
        assert_eq!(shorten("порт занят", 4), "порт…");
    }

    #[test]
    fn reason_prefers_line_that_explains() {
        let tail = |lines: &[&str]| lines.iter().map(|l| l.to_string()).collect::<Vec<_>>();

        // Как у панели REPLICATION: причина, а за ней подсказка.
        let busy = tail(&[
            "> node panel/server.js --open",
            "Порт 8765 уже занят: похоже, панель уже запущена. Адрес: http://127.0.0.1:8765",
            "Чтобы запустить вторую на другом порту, задайте переменную PANEL_PORT.",
        ]);
        assert!(pick_reason(&busy).unwrap().starts_with("Порт 8765 уже занят"));

        let node = tail(&["Error: listen EADDRINUSE: address already in use :::3000", "    at Server.setupListenHandle", "Node.js v24.19.0"]);
        assert!(pick_reason(&node).unwrap().contains("EADDRINUSE"));

        // Ничего похожего на причину: берём последние слова процесса.
        assert_eq!(pick_reason(&tail(&["building", "done"])).as_deref(), Some("done"));
        assert_eq!(pick_reason(&tail(&[])), None);
    }

    #[test]
    fn capitalize_handles_cyrillic() {
        assert_eq!(capitalize("папка не найдена".into()), "Папка не найдена");
        assert_eq!(capitalize(String::new()), "");
    }

    #[test]
    fn utf8_stream_survives_split_characters() {
        let bytes = "Сервер ✓".as_bytes();
        for cut in 0..=bytes.len() {
            let mut stream = Utf8Stream::default();
            let text = stream.feed(&bytes[..cut]) + &stream.feed(&bytes[cut..]);
            assert_eq!(text, "Сервер ✓", "разрез на байте {cut}");
        }
        // Битый байт не останавливает поток.
        let mut stream = Utf8Stream::default();
        assert_eq!(stream.feed(b"a\xFFb"), "a\u{FFFD}b");
        assert_eq!(stream.feed(b"c"), "c");
    }

    #[test]
    fn plain_line_drops_colors_and_redraws() {
        assert_eq!(plain_line("\u{1b}[32mready\u{1b}[39m\r\n"), "ready");
        assert_eq!(plain_line("10%\r50%\r100%\n"), "100%");
        assert_eq!(plain_line("\r\n"), "");
    }

    #[test]
    fn ansi_is_stripped() {
        assert_eq!(strip_ansi("\u{1b}[32m➜\u{1b}[39m  \u{1b}[1mLocal\u{1b}[22m"), "➜  Local");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}text"), "text");
    }

    #[test]
    fn url_found_in_vite_output() {
        let line = "  \u{1b}[32m➜\u{1b}[39m  \u{1b}[1mLocal\u{1b}[22m:   \u{1b}[36mhttp://localhost:\u{1b}[1m5173\u{1b}[22m/\u{1b}[39m";
        assert_eq!(find_local_url(line).as_deref(), Some("http://localhost:5173/"));
    }

    #[test]
    fn url_variants() {
        assert_eq!(find_local_url("  Web  http://localhost:7430").as_deref(), Some("http://localhost:7430"));
        assert_eq!(find_local_url("listening on http://0.0.0.0:8000.").as_deref(), Some("http://localhost:8000"));
        assert_eq!(find_local_url("API https://127.0.0.1:7431/health (ok)").as_deref(), Some("https://127.0.0.1:7431/health"));
        assert_eq!(find_local_url("open \"http://[::1]:3000/app\"").as_deref(), Some("http://[::1]:3000/app"));
    }

    #[test]
    fn foreign_urls_are_ignored() {
        assert_eq!(find_local_url("Network: http://192.168.1.20:5173/"), None);
        assert_eq!(find_local_url("see https://localhost.example.com/docs"), None);
        assert_eq!(find_local_url("docs at https://vite.dev then http://localhost:5173").as_deref(), Some("http://localhost:5173"));
        assert_eq!(find_local_url("no address here"), None);
    }
}
