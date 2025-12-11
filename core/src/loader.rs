use goblin::{elf, pe, mach, Object};
use std::path::Path;
use std::fs;
use crate::{TraceDB, Address};
use anyhow::{Result, Context};

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
                        }
                    }
                }
                println!("Loaded ELF binary: {:?}", path);
            },
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
                println!("Loaded PE binary: {:?}", path);
            },
            // Add Mach-O support if needed
            _ => println!("Unsupported binary format"),
        }
        
        Ok(())
    }
}
