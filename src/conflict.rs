//! Conflict markers in a file (`<<<<<<<`, optional diff3 `|||||||`, `=======`, `>>>>>>>`):
//! split into plain text and conflict blocks, and put a file back together from a choice per
//! block. Pure; the UI decides what "mine" means (during a rebase git's ours is upstream).

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Block(Block),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The label after `<<<<<<<` (e.g. `HEAD`) and after `>>>>>>>` (e.g. `clash`).
    pub ours_label: String,
    pub theirs_label: String,
    /// Lines with their line ends.
    pub ours: String,
    pub theirs: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Ours,
    Theirs,
    OursThenTheirs,
    TheirsThenOurs,
}

/// The parts of `text`, or `None` when it has no complete conflict block.
pub fn parse(text: &str) -> Option<Vec<Part>> {
    #[derive(PartialEq)]
    enum In {
        Text,
        Ours,
        Base,
        Theirs,
    }
    let mut parts = Vec::new();
    let mut text_buf = String::new();
    let mut block = Block {
        ours_label: String::new(),
        theirs_label: String::new(),
        ours: String::new(),
        theirs: String::new(),
    };
    let mut state = In::Text;
    let label = |line: &str, marker: &str| line[marker.len()..].trim().to_string();
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        match state {
            In::Text if bare.starts_with("<<<<<<<") => {
                if !text_buf.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut text_buf)));
                }
                block.ours_label = label(bare, "<<<<<<<");
                state = In::Ours;
            }
            In::Text => text_buf.push_str(line),
            In::Ours | In::Base if bare == "=======" => state = In::Theirs,
            In::Ours if bare.starts_with("|||||||") => state = In::Base,
            In::Ours => block.ours.push_str(line),
            In::Base => {}
            In::Theirs if bare.starts_with(">>>>>>>") => {
                block.theirs_label = label(bare, ">>>>>>>");
                parts.push(Part::Block(std::mem::replace(
                    &mut block,
                    Block {
                        ours_label: String::new(),
                        theirs_label: String::new(),
                        ours: String::new(),
                        theirs: String::new(),
                    },
                )));
                state = In::Text;
            }
            In::Theirs => block.theirs.push_str(line),
        }
    }
    // An unterminated block is not a conflict we can resolve.
    if state != In::Text {
        return None;
    }
    if !text_buf.is_empty() {
        parts.push(Part::Text(text_buf));
    }
    parts
        .iter()
        .any(|p| matches!(p, Part::Block(_)))
        .then_some(parts)
}

/// Number of conflict blocks.
pub fn blocks(parts: &[Part]) -> usize {
    parts.iter().filter(|p| matches!(p, Part::Block(_))).count()
}

/// The file with each block replaced by its choice; `None` while a block has none.
pub fn resolve(parts: &[Part], choices: &[Option<Choice>]) -> Option<String> {
    let mut out = String::new();
    let mut n = 0;
    for p in parts {
        match p {
            Part::Text(t) => out.push_str(t),
            Part::Block(b) => {
                let c = (*choices.get(n)?)?;
                n += 1;
                let (first, second) = match c {
                    Choice::Ours => (&b.ours, None),
                    Choice::Theirs => (&b.theirs, None),
                    Choice::OursThenTheirs => (&b.ours, Some(&b.theirs)),
                    Choice::TheirsThenOurs => (&b.theirs, Some(&b.ours)),
                };
                out.push_str(first);
                if let Some(s) = second {
                    // Keep the two sides on separate lines when the first lacks a line end.
                    if !out.is_empty() && !out.ends_with('\n') && !s.is_empty() {
                        out.push_str(if s.contains("\r\n") { "\r\n" } else { "\n" });
                    }
                    out.push_str(s);
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = "a\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> clash\nb\n\
                       <<<<<<< HEAD\nx\n||||||| base\nold\n=======\ny\ny2\n>>>>>>> clash\n";

    #[test]
    fn splits_text_and_blocks() {
        let parts = parse(TWO).unwrap();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], Part::Text("a\n".into()));
        let Part::Block(b) = &parts[1] else { panic!() };
        assert_eq!((b.ours.as_str(), b.theirs.as_str()), ("mine\n", "theirs\n"));
        assert_eq!(
            (b.ours_label.as_str(), b.theirs_label.as_str()),
            ("HEAD", "clash")
        );
        let Part::Block(b) = &parts[3] else { panic!() };
        assert_eq!(
            (b.ours.as_str(), b.theirs.as_str()),
            ("x\n", "y\ny2\n"),
            "diff3 base dropped"
        );
        assert_eq!(blocks(&parts), 2);
    }

    #[test]
    fn no_or_broken_markers_are_not_a_conflict() {
        assert_eq!(parse("plain\ntext\n"), None);
        assert_eq!(
            parse("a\n<<<<<<< HEAD\nmine\n=======\n"),
            None,
            "unterminated"
        );
        assert_eq!(parse("a\n======= not a marker\n"), None);
    }

    #[test]
    fn resolves_each_block_by_its_choice() {
        let parts = parse(TWO).unwrap();
        assert_eq!(resolve(&parts, &[Some(Choice::Ours), None]), None);
        assert_eq!(
            resolve(&parts, &[Some(Choice::Ours), Some(Choice::Theirs)]).unwrap(),
            "a\nmine\nb\ny\ny2\n"
        );
        assert_eq!(
            resolve(
                &parts,
                &[Some(Choice::TheirsThenOurs), Some(Choice::OursThenTheirs)]
            )
            .unwrap(),
            "a\ntheirs\nmine\nb\nx\ny\ny2\n"
        );
        // Windows line ends and a last line without one survive.
        let crlf = "<<<<<<< HEAD\r\nm\r\n=======\r\nt\r\n>>>>>>> b\r\nend";
        let parts = parse(crlf).unwrap();
        assert_eq!(
            resolve(&parts, &[Some(Choice::Theirs)]).unwrap(),
            "t\r\nend"
        );
    }
}
