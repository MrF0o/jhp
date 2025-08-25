#[derive(Debug)]
pub struct CodeBlockContent {
    pub lineno: usize,
    pub content: String,
    pub level: usize,
}

#[derive(Debug)]
pub enum CodeBlock {
    Html(CodeBlockContent),
    Javascript(CodeBlockContent),
    Expression(CodeBlockContent),
}

#[derive(Default, Debug)]
pub struct ParseResults {
    pub blocks: Vec<Box<CodeBlock>>,
}

impl ParseResults {
    fn add_block(&mut self, block: Box<CodeBlock>) {
        self.blocks.push(block);
    }
}

pub struct Parser<'a> {
    content: &'a str,
    pos: usize,
    line: usize,
    nesting: usize,
}

impl<'a> Parser<'a> {
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
            pos: 0,
            line: 1,
            nesting: 0,
        }
    }

    pub fn parse(&mut self) -> ParseResults {
        self.pos = 0;
        self.line = 1;
        self.nesting = 0;

        let mut results = ParseResults::default();
        while self.pos < self.content.len() {
            if self.lookahead("<?") {
                results.add_block(Box::new(self.parse_js_block()));
            } else {
                results.add_block(Box::new(self.parse_html_block()));
            }
        }
        results
    }

    pub fn set_content(&mut self, content: &'a str) {
        self.content = content;
        self.pos = 0;
        self.line = 1;
        self.nesting = 0;
    }

    fn parse_html_block(&mut self) -> CodeBlock {
        let start_line = self.line;
        let mut buf = String::new();

        while self.pos < self.content.len() && !self.lookahead("<?") {
            let c = self.consume();
            if c == '\n' {
                self.line += 1;
            }
            buf.push(c);
        }

        CodeBlock::Html(CodeBlockContent {
            lineno: start_line,
            content: buf,
            level: self.nesting,
        })
    }

    fn parse_js_block(&mut self) -> CodeBlock {
        let start_line = self.line;
        // opening "<?"
        let _ = self.consume(); // <
        let _ = self.consume(); // ?

        let mut buf = String::new();
        while self.pos < self.content.len() && !self.lookahead("?>") {
            let c = self.consume();
            if c == '\n' {
                self.line += 1;
            }
            buf.push(c);
        }

        if self.lookahead("?>") {
            let _ = self.consume(); // ?
            let _ = self.consume(); // >
        }

        let trimmed_start = buf.trim_start();
        let trimmed_end = buf.trim_end();
        if trimmed_start.starts_with('}') {
            self.nesting = self.nesting.saturating_sub(1);
        }

        let level = self.nesting;

        if trimmed_end.ends_with('{') {
            self.nesting += 1;
        }

        // expression block if it starts with '=' after leading whitespace
        if trimmed_start.starts_with('=') {
            let after_eq = trimmed_start[1..].trim();
            CodeBlock::Expression(CodeBlockContent {
                lineno: start_line,
                content: after_eq.to_string(),
                level,
            })
        } else {
            CodeBlock::Javascript(CodeBlockContent {
                lineno: start_line,
                content: buf,
                level,
            })
        }
    }

    fn lookahead(&self, pat: &str) -> bool {
        let bytes = self.content.as_bytes();
        let pat_bytes = pat.as_bytes();
        if self.pos + pat_bytes.len() > bytes.len() {
            return false;
        }
        &bytes[self.pos..self.pos + pat_bytes.len()] == pat_bytes
    }

    fn consume(&mut self) -> char {
        let s = &self.content[self.pos..];
        let mut iter = s.chars();
        let ch = iter.next().unwrap_or('\0');
        // advance by the byte length of this char
        self.pos += ch.len_utf8();
        ch
    }
}

/// Convert parsed JHP blocks into executable JavaScript source.
pub fn blocks_to_js<I>(blocks: I) -> String
where
    I: IntoIterator<Item = Box<CodeBlock>>,
{
    let mut js_lines: Vec<String> = Vec::new();

    for block in blocks {
        match *block {
            CodeBlock::Javascript(block) => {
                js_lines.push(block.content.trim().into());
            }
            CodeBlock::Html(block) => {
                js_lines.push(format!("html(`{}`);", block.content));
            }
            CodeBlock::Expression(block) => {
                js_lines.push(format!("html(String({}));", block.content.trim()));
            }
        }
    }

    js_lines.join("\n")
}
