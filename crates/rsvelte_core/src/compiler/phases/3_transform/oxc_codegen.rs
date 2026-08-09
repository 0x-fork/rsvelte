use std::path::PathBuf;

use oxc_allocator::{Allocator, CloneIn, Vec as ArenaVec};
use oxc_ast::ast::{BlockStatement, FunctionBody, Program, Statement, StaticBlock, SwitchCase};
use oxc_ast_visit::{VisitMut, walk_mut};
use oxc_codegen::{Codegen, CodegenOptions};

use super::js_ast::codegen::{CodegenResult, SourceMapping, build_line_starts, offset_to_line_col};

fn options(source_map: bool) -> CodegenOptions {
    CodegenOptions {
        single_quote: true,
        source_map_path: source_map.then(|| PathBuf::from("input.svelte")),
        ..CodegenOptions::default()
    }
}

fn without_final_newline(mut code: String) -> String {
    if code.ends_with('\n') {
        code.pop();
    }
    code
}

struct DropBareEmptyStatements;

impl DropBareEmptyStatements {
    fn retain_kept<'a>(statements: &mut ArenaVec<'a, Statement<'a>>) {
        let keep: std::vec::Vec<_> = statements
            .iter()
            .enumerate()
            .map(|(index, statement)| match statement {
                Statement::EmptyStatement(empty) => {
                    empty.span.end == u32::MAX
                        || (empty.span != oxc_span::SPAN
                            && (index.checked_sub(1).is_some_and(|previous| {
                                matches!(statements[previous], Statement::EmptyStatement(_))
                            }) || statements
                                .get(index + 1)
                                .is_some_and(|next| matches!(next, Statement::EmptyStatement(_)))))
                }
                _ => true,
            })
            .collect();
        let mut index = 0;
        statements.retain(|_| {
            let retain = keep[index];
            index += 1;
            retain
        });
    }
}

impl<'a> VisitMut<'a> for DropBareEmptyStatements {
    fn visit_program(&mut self, program: &mut Program<'a>) {
        walk_mut::walk_program(self, program);
        Self::retain_kept(&mut program.body);
    }

    fn visit_block_statement(&mut self, block: &mut BlockStatement<'a>) {
        walk_mut::walk_block_statement(self, block);
        Self::retain_kept(&mut block.body);
    }

    fn visit_switch_case(&mut self, case: &mut SwitchCase<'a>) {
        walk_mut::walk_switch_case(self, case);
        Self::retain_kept(&mut case.consequent);
    }

    fn visit_function_body(&mut self, body: &mut FunctionBody<'a>) {
        walk_mut::walk_function_body(self, body);
        Self::retain_kept(&mut body.statements);
    }

    fn visit_static_block(&mut self, block: &mut StaticBlock<'a>) {
        walk_mut::walk_static_block(self, block);
        Self::retain_kept(&mut block.body);
    }
}

pub fn print(program: &Program<'_>) -> String {
    let allocator = Allocator::default();
    let mut program = program.clone_in(&allocator);
    DropBareEmptyStatements.visit_program(&mut program);
    without_final_newline(
        Codegen::new()
            .with_options(options(false))
            .build(&program)
            .code,
    )
}

fn print_with_raw_map(program: &Program<'_>) -> CodegenResult {
    let allocator = Allocator::default();
    let mut program = program.clone_in(&allocator);
    DropBareEmptyStatements.visit_program(&mut program);
    let printed = Codegen::new().with_options(options(true)).build(&program);
    let mappings = printed
        .map
        .map(|map| {
            map.get_tokens()
                .filter_map(|token| {
                    token.get_source_id().map(|_| SourceMapping {
                        gen_line: token.get_dst_line(),
                        gen_col: token.get_dst_col(),
                        source: 0,
                        orig_line: token.get_src_line(),
                        orig_col: token.get_src_col(),
                        name: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    CodegenResult {
        code: without_final_newline(printed.code),
        mappings,
    }
}

pub fn print_with_map(program: &Program<'_>, original_source: &str) -> CodegenResult {
    let mut printed = print_with_raw_map(program);
    let source_starts = build_line_starts(program.source_text);
    printed.mappings.retain(|mapping| {
        line_col_to_offset(
            &source_starts,
            mapping.orig_line as usize,
            mapping.orig_col as usize,
        )
        .is_some_and(|offset| offset <= original_source.len())
    });
    printed
}

pub fn print_split_with_map(
    program: &Program<'_>,
    original_source: &str,
    loc_base: u32,
    loc_map: &[(u32, u32, Option<u32>)],
) -> CodegenResult {
    let mut printed = print_with_raw_map(program);
    let split_starts = build_line_starts(program.source_text);
    let original_starts = build_line_starts(original_source);

    printed.mappings.retain_mut(|mapping| {
        let Some(split_offset) = line_col_to_offset(
            &split_starts,
            mapping.orig_line as usize,
            mapping.orig_col as usize,
        ) else {
            return false;
        };
        if split_offset < loc_base as usize {
            return false;
        }
        let Some((start, _, Some(source_offset))) = loc_map
            .iter()
            .find(|(start, end, _)| (*start as usize..*end as usize).contains(&split_offset))
        else {
            return false;
        };
        let original_offset = *source_offset as usize + split_offset - *start as usize;
        if original_offset > original_source.len() {
            return false;
        }
        let (line, column) = offset_to_line_col(&original_starts, original_offset);
        mapping.source = 0;
        mapping.orig_line = line as u32;
        mapping.orig_col = column as u32;
        mapping.name = None;
        true
    });
    printed
}

fn line_col_to_offset(line_starts: &[usize], line: usize, column: usize) -> Option<usize> {
    line_starts.get(line).map(|start| start + column)
}
