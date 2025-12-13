use crate::{Address, TraceDB};
use anyhow::{Context, Result};
use goblin::{elf, mach, pe, Object};
use std::fs;
use std::path::Path;

pub struct BinaryLoader;

impl BinaryLoader {
    pub fn load_file(db: &TraceDB, path: &Path) -> Result<()> {
        let buffer = fs::read(path).context("Failed to read binary file")?;

        match Object::parse(&buffer)? {
            Object::Elf(elf) => {
                // Load loadable segments
                for ph in elf.program_headers {
                    if ph.p_type == elf::program_header::PT_LOAD {
                        let start = ph.p_vaddr;
                        let size = ph.p_filesz;
                        let offset = ph.p_offset as usize;

                        if size > 0 {
                            let data = &buffer[offset..offset + size as usize];
                            db.load_static_memory(start, data);

                            if ph.p_flags & elf::program_header::PF_X != 0 {
                                db.register_code_range(start, size);
                            }
                        }
                    }
                }
                
                // Load symbols
                for sym in elf.syms.iter() {
                    // Filter for functions
                    if sym.st_type() == elf::sym::STT_FUNC && sym.st_value != 0 {
                         if let Some(name) = elf.strtab.get_at(sym.st_name) {
                             // Use st_size if available, else 0
                             db.add_symbol(sym.st_value, sym.st_size, name.to_string());
                         }
                    }
                }

                db.set_entry_point(elf.header.e_entry);
                println!("Loaded ELF binary: {:?}", path);
            }
            Object::PE(pe) => {
                for section in pe.sections {
                    let start = pe.image_base as u64 + section.virtual_address as u64;
                    let size = section.size_of_raw_data;
                    let offset = section.pointer_to_raw_data as usize;

                    if size > 0 {
                        let data = &buffer[offset..offset + size as usize];
                        db.load_static_memory(start, data);
                    }
                }
                
                // Load PE exports as symbols
                for export in pe.exports {
                    if let Some(name) = export.name {
                        let addr = pe.image_base as u64 + export.rva as u64;
                         // PE exports usually don't have size info easily available here, use 0
                         db.add_symbol(addr, 0, name.to_string());
                    }
                }

                println!("Loaded PE binary: {:?}", path);
            }
            // Add Mach-O support if needed
            _ => println!("Unsupported binary format"),
        }

        Ok(())
    }
}
