// Copyright 2022 The Goscript Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

use super::branch::BranchHelper;
use super::codegen::*;
use super::consts::*;
use super::context::*;
use super::package::PkgHelper;
use super::types::{TypeCache, TypeLookup};
use go_parser::ast::Ident;
use go_parser::{AstObjects, ErrorList, FileSet, IdentKey, Map};
use go_types::{
    check::TypeInfo, ImportKey, Importer, PackageKey as TCPackageKey, SourceRead, TCObjects,
    TraceConfig,
};
use go_vm::types::*;
use go_vm::*;
use std::path::Path;
use std::vec;

pub fn parse_check_gen<S: SourceRead>(
    path: &Path,
    tconfig: &TraceConfig,
    reader: &S,
    fset: &mut FileSet,
) -> Result<Bytecode, ErrorList> {
    let ast_objs = &mut AstObjects::new();
    let tc_objs = &mut TCObjects::new();
    let results = &mut Map::new();
    let pkgs = &mut Map::new();
    let el = ErrorList::new();

    let importer = &mut Importer::new(
        &tconfig, reader, fset, pkgs, results, ast_objs, tc_objs, &el, 0,
    );
    let key = ImportKey::new(path.to_str().unwrap(), "./");
    let main_pkg = importer.import(&key);
    if el.len() > 0 {
        Err(el)
    } else {
        let blank_ident = ast_objs.idents.insert(Ident::blank(0));
        let main_ident = ast_objs.idents.insert(Ident::with_str(0, "main"));
        Ok(gen_byte_code(
            ast_objs,
            tc_objs,
            results,
            main_pkg.unwrap(),
            main_ident,
            blank_ident,
        ))
    }
}

fn gen_byte_code(
    ast_objs: &AstObjects,
    tc_objs: &TCObjects,
    checker_result: &Map<TCPackageKey, TypeInfo>,
    tc_main_pkg: TCPackageKey,
    main_ident: IdentKey,
    blank_ident: IdentKey,
) -> Bytecode {
    let vm_objs = VMObjects::new();
    let mut vmctx = CodeGenVMCtx::new(vm_objs);
    let consts = Consts::new();
    let mut iface_selector = IfaceSelector::new();
    let mut struct_selector = StructSelector::new();
    let mut pkg_map = Map::new();
    let mut type_cache: TypeCache = Map::new();
    let mut branch_helper = BranchHelper::new();
    let mut result_funcs = vec![];

    for (&tcpkg, _) in checker_result.iter() {
        let name = tc_objs.pkgs[tcpkg].name().clone().unwrap();
        let pkey = vmctx.packages_mut().insert(PackageObj::new(name));
        pkg_map.insert(tcpkg, pkey);
    }

    let main_pkg = pkg_map[&tc_main_pkg];
    let entry = gen_entry_func(&mut vmctx, &consts, main_pkg, main_ident);
    let entry_key = entry.f_key;
    result_funcs.push(entry);

    for (tcpkg, ti) in checker_result.iter() {
        let mut pkg_helper = PkgHelper::new(ast_objs, tc_objs, &pkg_map);
        let cgen = CodeGen::new(
            &mut vmctx,
            &consts,
            ast_objs,
            tc_objs,
            &ti,
            &mut type_cache,
            &mut iface_selector,
            &mut struct_selector,
            &mut branch_helper,
            &mut pkg_helper,
            pkg_map[tcpkg],
            blank_ident,
        );
        result_funcs.append(&mut cgen.gen_with_files(&ti.ast_files, *tcpkg));
    }

    let (consts, cst_map) = consts.get_runtime_consts(&mut vmctx);
    for f in result_funcs.into_iter() {
        f.into_runtime_func(ast_objs, &mut vmctx, branch_helper.labels(), &cst_map);
    }

    let dummy_ti = TypeInfo::new();
    let mut lookup = TypeLookup::new(tc_objs, &dummy_ti, &mut type_cache);
    let iface_binding = iface_selector
        .result()
        .into_iter()
        .map(|x| lookup.iface_binding_info(x, &mut vmctx))
        .collect();

    Bytecode::new(
        vmctx.into_vmo(),
        consts,
        iface_binding,
        struct_selector.result(),
        entry_key,
        main_pkg,
    )
}

// generate the entry function for Bytecode
fn gen_entry_func<'a, 'c>(
    vmctx: &'a mut CodeGenVMCtx,
    consts: &'c Consts,
    pkg: PackageKey,
    main_ident: IdentKey,
) -> FuncCtx<'c> {
    let fmeta = vmctx.prim_meta().default_sig;
    let fobj = vmctx.function_with_meta(None, fmeta.clone(), FuncFlag::Default);
    let fkey = *fobj.as_function();
    let mut fctx = FuncCtx::new(fkey, None, consts);
    fctx.emit_import(pkg, None);
    let pkg_addr = fctx.add_package(pkg);
    let index = Addr::PkgMemberIndex(pkg, main_ident);
    fctx.emit_load_pkg(Addr::Regsiter(0), pkg_addr, index, None);
    fctx.emit_call(Addr::Regsiter(0), 0, CallStyle::Default, None);
    fctx.emit_return(None, None, vmctx.functions());
    fctx
}
