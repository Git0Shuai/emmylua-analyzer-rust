mod common;
mod decl;
mod doc;
mod flow;
mod infer_cache_manager;
mod lua;
mod unresolve;

use std::{collections::HashMap, sync::Arc};

use crate::{Emmyrc, InFiled, InferFailReason, WorkspaceId, db_index::DbIndex, profile::Profile};
use emmylua_parser::LuaChunk;
use infer_cache_manager::InferCacheManager;
use unresolve::UnResolve;

pub fn analyze(db: &mut DbIndex, need_analyzed_files: Vec<InFiled<LuaChunk>>, config: Arc<Emmyrc>) {
    if need_analyzed_files.is_empty() {
        return;
    }

    let mut contexts = module_analyze(db, need_analyzed_files, config);

    for (_, context) in &mut contexts {
        run_analysis::<KgRequireMarkPipeline>(db, context);
    }
    for (workspace_id, mut context) in contexts {
        let profile_log = format!("analyze workspace {}", workspace_id);
        let _p = Profile::cond_new(&profile_log, context.tree_list.len() > 1);
        run_analysis::<decl::DeclAnalysisPipeline>(db, &mut context);
        run_analysis::<doc::DocAnalysisPipeline>(db, &mut context);
        run_analysis::<flow::FlowAnalysisPipeline>(db, &mut context);
        run_analysis::<lua::LuaAnalysisPipeline>(db, &mut context);
        run_analysis::<unresolve::UnResolveAnalysisPipeline>(db, &mut context);
    }
}

struct KgRequireMarkPipeline;

impl AnalysisPipeline for KgRequireMarkPipeline {
    fn analyze(_db: &mut DbIndex, _context: &mut AnalyzeContext) {
        // 根据 kg_require 调用分析模块执行上下文是否为自定义 fenv
        // for tree in context.tree_list.iter() {
        //     for walk_event in tree.value.walk_descendants::<LuaAst>() {
        //         match walk_event {
        //             rowan::WalkEvent::Enter(node) => {
        //                 if let LuaAst::LuaCallExpr(call_expr) = node
        //                     && (call_expr.is_kg_require())
        //                 {
        //                     if let Some(args) = call_expr.get_args_list() {
        //                         if let Some(LuaExpr::LiteralExpr(module_path_expr)) =
        //                             args.get_args().next()
        //                         {
        //                             if let Some(LuaLiteralToken::String(literal_str_token)) =
        //                                 module_path_expr.get_literal()
        //                             {
        //                                 if let Some(required_module_info) = db
        //                                     .get_module_index()
        //                                     .find_module(&literal_str_token.get_value())
        //                                 {
        //                                     let file_id = required_module_info.file_id;
        //                                     db.get_module_index_mut().set_kg_required(&file_id);
        //                                 }
        //                             }
        //                         }
        //                     }
        //                 }
        //             }
        //             _ => {}
        //         }
        //     }
        // }
    }
}

trait AnalysisPipeline {
    fn analyze(db: &mut DbIndex, context: &mut AnalyzeContext);
}

fn run_analysis<T: AnalysisPipeline>(db: &mut DbIndex, context: &mut AnalyzeContext) {
    T::analyze(db, context);
}

fn module_analyze(
    db: &mut DbIndex,
    need_analyzed_files: Vec<InFiled<LuaChunk>>,
    config: Arc<Emmyrc>,
) -> Vec<(WorkspaceId, AnalyzeContext)> {
    if need_analyzed_files.len() == 1 {
        let in_filed_tree = need_analyzed_files[0].clone();
        let file_id = in_filed_tree.file_id;
        if let Some(path) = db.get_vfs().get_file_path(&file_id).cloned() {
            let path_str = match path.to_str() {
                Some(path) => path,
                None => {
                    log::warn!("file_id {:?} path not found", file_id);
                    return vec![];
                }
            };

            let workspace_id = db
                .get_module_index_mut()
                .add_module_by_path(file_id, path_str);
            let workspace_id = workspace_id.unwrap_or(WorkspaceId::MAIN);
            let mut context = AnalyzeContext::new(config);
            context.add_tree_chunk(in_filed_tree);
            return vec![(workspace_id, context)];
        }

        return vec![];
    }

    let _p = Profile::new("module analyze");
    let mut file_tree_map: HashMap<WorkspaceId, Vec<InFiled<LuaChunk>>> = HashMap::new();
    for in_filed_tree in need_analyzed_files {
        let file_id = in_filed_tree.file_id;
        if let Some(path) = db.get_vfs().get_file_path(&file_id).cloned() {
            let path_str = match path.to_str() {
                Some(path) => path,
                None => {
                    log::warn!("file_id {:?} path not found", file_id);
                    continue;
                }
            };

            let workspace_id = db
                .get_module_index_mut()
                .add_module_by_path(file_id, path_str);
            let workspace_id = workspace_id.unwrap_or(WorkspaceId::MAIN);
            file_tree_map
                .entry(workspace_id)
                .or_default()
                .push(in_filed_tree);
        }
    }

    let mut contexts = Vec::new();
    if let Some(std_lib) = file_tree_map.remove(&WorkspaceId::STD) {
        let mut context = AnalyzeContext::new(config.clone());
        context.tree_list = std_lib;
        contexts.push((WorkspaceId::STD, context));
    }

    let mut main_vec = Vec::new();
    for (workspace_id, tree_list) in file_tree_map {
        let mut context = AnalyzeContext::new(config.clone());
        context.tree_list = tree_list;
        if workspace_id.is_library() {
            contexts.push((workspace_id, context));
        } else {
            main_vec.push((workspace_id, context));
        }
    }

    contexts.sort_by(|a, b| a.0.cmp(&b.0));

    contexts.extend(main_vec);
    contexts
}

#[derive(Debug)]
pub struct AnalyzeContext {
    tree_list: Vec<InFiled<LuaChunk>>,
    #[allow(unused)]
    config: Arc<Emmyrc>,
    unresolves: Vec<(UnResolve, InferFailReason)>,
    infer_manager: InferCacheManager,
}

impl AnalyzeContext {
    pub fn new(emmyrc: Arc<Emmyrc>) -> Self {
        Self {
            tree_list: Vec::new(),
            config: emmyrc,
            unresolves: Vec::new(),
            infer_manager: InferCacheManager::new(),
        }
    }

    pub fn add_tree_chunk(&mut self, tree: InFiled<LuaChunk>) {
        self.tree_list.push(tree);
    }

    pub fn add_unresolve(&mut self, un_resolve: UnResolve, reason: InferFailReason) {
        self.unresolves.push((un_resolve, reason));
    }
}
