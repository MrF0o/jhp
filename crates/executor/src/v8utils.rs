//! Shared V8 utilities to reduce boilerplate and keep hot paths fast.
use std::sync::{Arc, Mutex};

use jhp_parser::{CodeBlock, CodeBlockContent};

/// Compile and run JavaScript `code` in the current context of the provided HandleScope.
/// `resource_name` is used for stack traces and debugging.
pub fn compile_and_run_current<'h>(
    hs: &mut v8::HandleScope<'h>,
    code: &str,
    resource_name: &str,
) -> Result<(), String> {
    let context = hs.get_current_context();
    let mut scope = v8::ContextScope::new(hs, context);
    let source =
        v8::String::new(&mut scope, code).ok_or_else(|| "Failed to create source".to_string())?;
    let name = v8::String::new(&mut scope, resource_name)
        .ok_or_else(|| "Failed to create resource name".to_string())?;
    let origin: v8::ScriptOrigin<'_> = v8::ScriptOrigin::new(
        &mut scope,
        name.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    );
    let script = v8::Script::compile(&mut scope, source, Some(&origin))
        .ok_or_else(|| "Failed to compile script".to_string())?;
    script
        .run(&mut scope)
        .ok_or_else(|| "Failed to run script".to_string())?;
    Ok(())
}

/// Compile JavaScript into a Script using the given ContextScope and resource name.
pub fn compile_script<'s>(
    scope: &mut v8::ContextScope<v8::HandleScope<'s>>,
    code: &str,
    resource_name: &str,
) -> Result<v8::Local<'s, v8::Script>, String> {
    let source =
        v8::String::new(scope, code).ok_or_else(|| "Failed to create source".to_string())?;
    let name = v8::String::new(scope, resource_name)
        .ok_or_else(|| "Failed to create resource name".to_string())?;
    let origin = v8::ScriptOrigin::new(
        scope,
        name.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    );
    v8::Script::compile(scope, source, Some(&origin))
        .ok_or_else(|| "Failed to compile a script block".to_string())
}

/// Execute parsed JHP blocks one-by-one with per-block ScriptOrigin for accurate
/// line/column reporting. If an error occurs, append a formatted stack trace to the
/// provided output buffer and return Err.
pub fn run_jhp_blocks_with_origin<'h>(
    hs: &mut v8::HandleScope<'h>,
    blocks: Vec<Box<CodeBlock>>,
    resource_name: &str,
    output_buffer: Arc<Mutex<String>>,
) -> Result<(), String> {
    for block in blocks {
        match *block {
            CodeBlock::Html(CodeBlockContent { content, .. }) => {
                let src = format!("echo(`{}`);", content);
                if let Err(e) = compile_and_run_current_with_origin(hs, &src, resource_name, 0, 0) {
                    push_error(&output_buffer, &e);
                    return Err(e);
                }
            }
            CodeBlock::Expression(CodeBlockContent { content, .. }) => {
                let src = format!("echo(String({}));", content.trim());
                // Column offset equals prefix length before the expression
                let col_off = 12; // len("echo(String(") = 12
                if let Err(e) =
                    compile_and_run_current_with_origin(hs, &src, resource_name, 0, col_off)
                {
                    push_error(&output_buffer, &e);
                    return Err(e);
                }
            }
            CodeBlock::Javascript(CodeBlockContent {
                content, lineno, ..
            }) => {
                // Adjust origin starting line to the block's starting line (1-based)
                if let Err(e) = compile_and_run_current_with_origin(
                    hs,
                    content.trim(),
                    resource_name,
                    lineno as i32 - 1,
                    0,
                ) {
                    push_error(&output_buffer, &e);
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}

/// Compile and run in current context with specific origin line/column offsets.
pub fn compile_and_run_current_with_origin<'h>(
    hs: &mut v8::HandleScope<'h>,
    code: &str,
    resource_name: &str,
    line_offset: i32,
    column_offset: i32,
) -> Result<(), String> {
    let tc = &mut v8::TryCatch::new(hs);
    let context = tc.get_current_context();
    let mut cscope = v8::ContextScope::new(tc, context);
    let source =
        v8::String::new(&mut cscope, code).ok_or_else(|| "Failed to create source".to_string())?;
    let name = v8::String::new(&mut cscope, resource_name)
        .ok_or_else(|| "Failed to create resource name".to_string())?;
    let origin = v8::ScriptOrigin::new(
        &mut cscope,
        name.into(),
        line_offset,
        column_offset,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    );
    let mut had_error = false;
    if let Some(script) = v8::Script::compile(&mut cscope, source, Some(&origin)) {
        if script.run(&mut cscope).is_none() {
            had_error = true;
        }
    } else {
        had_error = true;
    }
    drop(cscope); // release borrow before inspecting tc
    if had_error {
        Err(format_v8_exception(tc, resource_name))
    } else {
        Ok(())
    }
}

fn push_error(buffer: &Arc<Mutex<String>>, err: &str) {
    let msg = format!("\n<!-- ERROR -->\n{}\n", err);
    if let Ok(mut guard) = buffer.lock() {
        guard.push_str(&msg);
    }
}

fn format_v8_exception(scope: &mut v8::TryCatch<v8::HandleScope>, fallback_name: &str) -> String {
    let exception_str = scope
        .exception()
        .and_then(|e| e.to_string(scope.as_mut()))
        .map(|s| s.to_rust_string_lossy(scope.as_mut()))
        .unwrap_or_else(|| "Uncaught exception".to_string());

    let (resource_name, line, column) = if let Some(msg) = scope.message() {
        let name = msg
            .get_script_resource_name(scope.as_mut())
            .and_then(|v| v.to_string(scope.as_mut()))
            .map(|s| s.to_rust_string_lossy(scope.as_mut()))
            .unwrap_or_else(|| fallback_name.to_string());
        let line = msg.get_line_number(scope.as_mut()).unwrap_or(0);
        let column = msg.get_start_column();
        (name, line, column)
    } else {
        (fallback_name.to_string(), 0, 0)
    };

    let stack = scope
        .stack_trace()
        .and_then(|v| v8::Local::<v8::String>::try_from(v).ok())
        .map(|s| s.to_rust_string_lossy(scope.as_mut()))
        .unwrap_or_default();

    let header = format!("{}:{}:{}", resource_name, line, column);
    let stack_trim = stack.trim();
    if !stack_trim.is_empty() {
        // If the stack already starts with the exception summary, don't duplicate it.
        if stack_trim.starts_with(&exception_str) {
            format!("{}\n{}", header, stack_trim)
        } else {
            format!("{}\n{}\n{}", header, exception_str, stack_trim)
        }
    } else {
        format!("{}\n{}", header, exception_str)
    }
}
