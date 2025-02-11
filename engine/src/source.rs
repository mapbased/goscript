// Copyright 2022 The Goscript Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

use crate::engine::{Config, Engine, SourceRead};
use crate::vfs::VirtualFs;
use crate::ErrorList;
use go_parser::Map;
use std::io;
use std::path::{Path, PathBuf};

const VIRTUAL_LOCAL_PATH_PREFIX: &str = "vfs_local_";

pub fn run(config: Config, source: &SourceReader, path: &Path) -> Result<(), ErrorList> {
    let engine = Engine::new();
    #[cfg(feature = "go_std")]
    engine.set_std_io(config.std_in, config.std_out, config.std_err);
    engine.run_source(config.trace_parser, config.trace_checker, source, path)
}

pub struct SourceReader {
    /// base directory for non-local imports(library files)
    base_dir: Option<PathBuf>,
    /// working directory
    working_dir: PathBuf,
    /// The virtual file system from which to read files.
    vfs: Box<dyn VirtualFs>,
}

impl SourceReader {
    pub fn new(
        base_dir: Option<PathBuf>,
        working_dir: PathBuf,
        vfs: Box<dyn VirtualFs>,
    ) -> SourceReader {
        SourceReader {
            base_dir,
            working_dir,
            vfs,
        }
    }

    /// Create a SourceReader that reads from local file system.
    #[cfg(feature = "read_fs")]
    pub fn local_fs(base_dir: PathBuf, working_dir: PathBuf) -> SourceReader {
        SourceReader::new(Some(base_dir), working_dir, Box::new(crate::VfsFs {}))
    }

    /// Create a SourceReader that reads from a zip file and a string.
    #[cfg(feature = "read_fs")]
    pub fn fs_lib_and_string(
        base_dir: PathBuf,
        source: std::borrow::Cow<'static, str>,
    ) -> (SourceReader, PathBuf) {
        let temp_file_name = "temp_file.gos";
        // must start with VIRTUAL_LOCAL_PATH_PREFIX to be recognized as a local(as opposed to lib file) file
        let vfs_map_name = format!("{}map", VIRTUAL_LOCAL_PATH_PREFIX);
        let vfs_fs_name = "vfs_fs";
        (
            SourceReader::new(
                Some(Path::new(vfs_fs_name).join(base_dir)),
                PathBuf::from(format!("{}/", vfs_map_name)),
                Box::new(crate::CompoundFs::new(Map::from([
                    (
                        vfs_fs_name.to_owned(),
                        Box::new(crate::VfsFs {}) as Box<dyn VirtualFs>,
                    ),
                    (
                        vfs_map_name.to_owned(),
                        Box::new(crate::VfsMap::new(Map::from([(
                            PathBuf::from(temp_file_name),
                            source,
                        )]))),
                    ),
                ]))),
            ),
            PathBuf::from(format!("./{}", temp_file_name)),
        )
    }

    /// Creates a SourceReader that reads from a zip archive and a string.
    /// Returns the SourceReader and the path of the virtual file that contains the string.
    #[cfg(feature = "read_zip")]
    pub fn zip_lib_and_string(
        archive: std::borrow::Cow<'static, [u8]>,
        base_dir: PathBuf,
        source: std::borrow::Cow<'static, str>,
    ) -> (SourceReader, PathBuf) {
        let temp_file_name = "temp_file.gos";
        // must start with VIRTUAL_LOCAL_PATH_PREFIX to be recognized as a local(as opposed to lib file) file
        let vfs_map_name = format!("{}map", VIRTUAL_LOCAL_PATH_PREFIX);
        let vfs_zip_name = "vfs_zip";
        (
            SourceReader::new(
                Some(Path::new(vfs_zip_name).join(base_dir)),
                PathBuf::from(format!("{}/", vfs_map_name)),
                Box::new(crate::CompoundFs::new(Map::from([
                    (
                        vfs_zip_name.to_owned(),
                        Box::new(crate::VfsZip::new(archive).unwrap()) as Box<dyn VirtualFs>,
                    ),
                    (
                        vfs_map_name.to_owned(),
                        Box::new(crate::VfsMap::new(Map::from([(
                            PathBuf::from(temp_file_name),
                            source,
                        )]))),
                    ),
                ]))),
            ),
            PathBuf::from(format!("./{}", temp_file_name)),
        )
    }

    /// Creates a SourceReader that reads from a zip archive and local file system.
    /// Can be used when you want to read library files from a zip archive and user's
    /// source code from the local file system.
    #[cfg(feature = "read_zip")]
    pub fn zip_lib_and_local_fs(
        archive: std::borrow::Cow<'static, [u8]>,
        base_dir: PathBuf,
        working_dir: PathBuf,
    ) -> SourceReader {
        // must start with VIRTUAL_LOCAL_PATH_PREFIX to be recognized as a local(as opposed to lib file) file
        let vfs_fs_name = format!("{}local_fs", VIRTUAL_LOCAL_PATH_PREFIX);
        let vfs_zip_name = "vfs_zip";

        SourceReader::new(
            Some(Path::new(vfs_zip_name).join(base_dir)),
            Path::new(&vfs_fs_name).join(working_dir),
            Box::new(crate::CompoundFs::new(Map::from([
                (vfs_fs_name, Box::new(crate::VfsFs {}) as Box<dyn VirtualFs>),
                (
                    vfs_zip_name.to_owned(),
                    Box::new(crate::VfsZip::new(archive).unwrap()) as Box<dyn VirtualFs>,
                ),
            ]))),
        )
    }
}

impl SourceRead for SourceReader {
    fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    fn base_dir(&self) -> Option<&Path> {
        self.base_dir.as_ref().map(|x| x.as_path())
    }

    fn read_file(&self, path: &Path) -> io::Result<String> {
        self.vfs.read_file(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.vfs.read_dir(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.vfs.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.vfs.is_dir(path)
    }

    fn canonicalize_path(&self, path: &PathBuf) -> io::Result<PathBuf> {
        self.vfs.canonicalize_path(path)
    }
}
