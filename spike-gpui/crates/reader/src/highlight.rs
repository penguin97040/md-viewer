//! Syntax highlighting with syntect, mapped onto the chroma token classes the web reader's colours
//! are defined on. Same limits as `render.go`: 256 KB per block and a 1 s budget.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::model::{ByteSpan, CodeToken, TokenClass as T};

const MAX_BYTES: usize = 256 * 1024;
const BUDGET: Duration = Duration::from_secs(1);

fn set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Loads the grammars on a background thread, so the first code block does not stall.
pub fn prewarm() {
    let _ = set();
}

fn syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let s = set();
    // Fences name languages in ways the bundled grammars do not list. TypeScript has no grammar
    // here, so it borrows JavaScript's.
    let lang = match lang {
        "ts" | "tsx" | "typescript" | "jsx" | "mjs" | "cjs" => "js",
        "shell" | "console" | "zsh" | "sh" => "bash",
        "golang" => "go",
        "py3" | "python3" => "python",
        "rs" => "rust",
        "yml" => "yaml",
        "c++" => "cpp",
        "objc" => "m",
        l => l,
    };
    s.find_syntax_by_token(lang).or_else(|| s.find_syntax_by_name(lang))
}

/// The token class for one TextMate scope, or `None` to look further down the stack.
fn class_of(scope: &str) -> Option<T> {
    let starts = |p: &str| scope == p || (scope.starts_with(p) && scope.as_bytes().get(p.len()) == Some(&b'.'));
    Some(if starts("comment") || starts("punctuation.definition.comment") {
        T::Comment
    } else if starts("meta.preprocessor") || starts("keyword.control.import.include") {
        T::CommentPreproc
    } else if starts("string.regexp") {
        T::StringRegex
    } else if starts("constant.character.escape") {
        T::StringEscape
    } else if starts("punctuation.definition.string") || starts("string") || starts("constant.character") {
        T::String
    } else if starts("constant.other.symbol") {
        T::StringSymbol
    } else if starts("constant.numeric") {
        T::Number
    } else if starts("constant.language") {
        T::KeywordConstant
    } else if starts("constant") || starts("support.constant") || starts("entity.name.constant") {
        T::NameConstant
    } else if starts("keyword.operator.word") || starts("keyword.operator.logical.python") {
        T::Operator
    } else if starts("keyword.operator") {
        T::Operator
    } else if starts("keyword.control.import") || starts("keyword.other.import") {
        T::KeywordNamespace
    } else if starts("keyword") {
        T::Keyword
    } else if starts("storage.type.function") || starts("storage.type.class") || starts("storage") {
        T::KeywordDeclaration
    } else if starts("support.type") {
        T::KeywordType
    } else if starts("entity.name.function") || starts("support.function.magic") {
        T::NameFunction
    } else if starts("support.function") || starts("variable.function.builtin") {
        T::NameBuiltin
    } else if starts("support.class") || starts("entity.name.class") || starts("entity.name.type") || starts("entity.name.struct") || starts("entity.name.enum") || starts("entity.other.inherited-class") {
        T::NameClass
    } else if starts("entity.name.namespace") || starts("entity.name.package") || starts("entity.name.module") {
        T::NameNamespace
    } else if starts("entity.name.tag") {
        T::NameTag
    } else if starts("entity.other.attribute-name") {
        T::NameAttribute
    } else if starts("entity.name.label") {
        T::NameLabel
    } else if starts("entity.name.decorator") || starts("meta.annotation") {
        T::NameDecorator
    } else if starts("variable.language") {
        T::NameBuiltinPseudo
    } else if starts("variable.other.member") || starts("variable.other.property") {
        T::NameProperty
    } else if starts("variable") || starts("entity.name") {
        T::Name
    } else if starts("markup.inserted") {
        T::GenericInserted
    } else if starts("markup.deleted") {
        T::GenericDeleted
    } else if starts("markup.heading") {
        T::GenericHeading
    } else if starts("markup.italic") {
        T::GenericEmph
    } else if starts("markup.bold") {
        T::GenericStrong
    } else if starts("invalid") {
        T::Error
    } else if starts("punctuation") {
        T::Punctuation
    } else {
        return None;
    })
}

thread_local! {
    static MEMO: RefCell<HashMap<Scope, Option<T>>> = RefCell::new(HashMap::new());
}

fn class_of_stack(stack: &ScopeStack) -> Option<T> {
    MEMO.with(|m| {
        let mut m = m.borrow_mut();
        for s in stack.as_slice().iter().rev() {
            let c = *m.entry(*s).or_insert_with(|| class_of(&s.build_string()));
            if c.is_some() {
                return c;
            }
        }
        None
    })
}

/// Tokens for one code block. `base` is the code's start in the flat text. Empty when the language
/// is unknown, the block is too big, or highlighting runs over budget, as in the web reader.
pub fn highlight(lang: &str, code: &str, base: u32) -> Vec<CodeToken> {
    if lang.is_empty() || code.len() > MAX_BYTES {
        return Vec::new();
    }
    let Some(syn) = syntax(lang) else { return Vec::new() };
    let deadline = Instant::now() + BUDGET;
    let s = set();
    let mut state = ParseState::new(syn);
    let mut stack = ScopeStack::new();
    let mut out: Vec<CodeToken> = Vec::new();
    let mut at = 0usize;
    for line in code.split_inclusive('\n') {
        if Instant::now() > deadline {
            return Vec::new();
        }
        let Ok(ops) = state.parse_line(line, s) else { return Vec::new() };
        let mut last = 0usize;
        let emit = |out: &mut Vec<CodeToken>, from: usize, to: usize, stack: &ScopeStack| {
            if from >= to {
                return;
            }
            if let Some(class) = class_of_stack(stack) {
                let (a, b) = (base + (at + from) as u32, base + (at + to) as u32);
                match out.last_mut() {
                    Some(t) if t.class == class && t.span.end == a => t.span.end = b,
                    _ => out.push(CodeToken { span: ByteSpan::new(a, b), class }),
                }
            }
        };
        for (pos, op) in ops {
            emit(&mut out, last, pos, &stack);
            last = pos;
            if stack.apply(&op).is_err() {
                return Vec::new();
            }
        }
        emit(&mut out, last, line.len(), &stack);
        at += line.len();
    }
    // Stay within the flat text: the block's final newline is not in `code` when the source had
    // none, and tokens may run to its very end.
    let end = base + code.len() as u32;
    out.retain_mut(|t| {
        t.span.end = t.span.end.min(end);
        !t.span.is_empty()
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(lang: &str, code: &str) -> Vec<(String, T)> {
        highlight(lang, code, 0).into_iter().map(|t| (code[t.span.range()].to_string(), t.class)).collect()
    }

    #[test]
    fn go_function() {
        let c = classes("go", "func main() {}\n");
        assert!(c.iter().any(|(s, k)| s == "func" && matches!(k, T::Keyword | T::KeywordDeclaration)), "{c:?}");
        assert!(c.iter().any(|(s, k)| s == "main" && *k == T::NameFunction), "{c:?}");
    }

    #[test]
    fn strings_and_comments() {
        let c = classes("js", "// note\nconst a = 'x';\n");
        assert!(c.iter().any(|(s, k)| s.starts_with("// note") && *k == T::Comment), "{c:?}");
        assert!(c.iter().any(|(s, k)| s == "'x'" && *k == T::String), "{c:?}");
    }

    #[test]
    fn unknown_language_and_oversize_are_plain() {
        assert!(highlight("nosuchlanguage", "x", 0).is_empty());
        assert!(highlight("", "x", 0).is_empty());
        assert!(highlight("js", &"x".repeat(MAX_BYTES + 1), 0).is_empty());
    }

    #[test]
    fn offsets_include_the_base() {
        let t = highlight("js", "'x'", 10);
        assert_eq!(t[0].span, ByteSpan::new(10, 13));
    }
}
