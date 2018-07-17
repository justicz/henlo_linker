extern crate object;
extern crate byteorder;

use std::io::prelude::*;
use std::io::Cursor;
use std::collections::HashMap;
use std::{env, fs, process};
use object::Object;
use byteorder::{ReadBytesExt, LittleEndian};

const CLEAR_R_AC: u16 = 0b0110110110110000;
const SHIFT_R_AC: u16 = 0b0011110100000000;
const JMP_R_AC: u16 = 0b1011110000000000;
const ADDI_R_AC: u8 = 0b00011100;
const HLT: u8 = 0b11111111;
const ENTRY: &str = "main";
const ENTRYPOINT_LEN: usize = 10;
const INSTRUCTION_BYTES: usize = 2;

fn load_addr_code(addr: usize) -> Vec<u8> {
    if addr > std::u16::MAX as usize {
        panic!("Address too big");
    }

    let top = (addr >> 8) as u8;
    let bottom = (addr & 0xFF) as u8;

    let mut v = Vec::<u8>::new();

    // Clear out R_AC
    v.push((CLEAR_R_AC >> 8) as u8);
    v.push((CLEAR_R_AC & 0xFF) as u8);

    // Set top bits
    v.push(ADDI_R_AC);
    v.push(top);

    // Shift R_AC
    v.push((SHIFT_R_AC >> 8) as u8);
    v.push((SHIFT_R_AC & 0xFF) as u8);

    // Set bottom bits
    v.push(ADDI_R_AC);
    v.push(bottom);

    return v;
}

fn gen_entrypoint(addr: usize) -> Vec<u8> {
    let mut ac = load_addr_code(addr);
    ac.push((JMP_R_AC >> 8) as u8);
    ac.push((JMP_R_AC & 0xFF) as u8);
    return ac;
}

fn get_object_file<'a>(file_path: &str, buffer: &'a mut Vec<u8>) -> Option<object::File<'a>> {
    let mut file = match fs::File::open(file_path) {
        Ok(file) => file,
        Err(err) => {
            println!("Failed to open file '{}': {}", file_path, err,);
            return None;
        }
    };

    match file.read_to_end(buffer) {
        Ok(_) => (),
        Err(err) => {
            println!("Failed to open file '{}': {}", file_path, err,);
            return None;
        }
    }

    match object::File::parse(buffer) {
        Ok(file) => return Some(file),
        Err(err) => {
            println!("Failed to parse file '{}': {}", file_path, err);
            return None;
        }
    };
}

#[derive(Debug)]
struct SimpleSymbol {
    name: Option<String>,
    symbol_index: usize,
    address: usize,
    size: usize
}

#[derive(Debug)]
struct WriteSymbol {
    symbol_index: usize,
    cs_offset: usize
}

fn main() {
    let arg_len = env::args().len();
    if arg_len <= 1 {
        eprintln!("Usage: {} <file> ...", env::args().next().unwrap());
        process::exit(1);
    }

    // let mut executable = Vec::new();
    let mut write_what_where = Vec::new();
    let mut simple_symbols = Vec::new();

    let mut symbol_addresses = HashMap::new();
    let mut executable = Vec::<u8>::new();

    for file_path in env::args().skip(1) {
        // object::File has the same lifetime as the data passed to parse, so
        // this buffer needs to live
        let mut buffer = Vec::new();
        let file = match get_object_file(&file_path, &mut buffer) {
            Some(file) => file,
            None => return
        };

        let symbol_offset = simple_symbols.len();
        let exec_offset = executable.len();
        let mut main_last_instruction_offset: Option<usize> = None;

        for symbol in file.symbols() {
            let mut ss = SimpleSymbol {
                name: None,
                symbol_index: simple_symbols.len(),
                address: symbol.address() as usize,
                size: symbol.size() as usize
            };

            match symbol.name() {
                Some(ref name) => {
                    ss.name = Some(String::from(*name));

                    match symbol.section_kind() {
                        Some(_) => {
                            match symbol_addresses.get(*name) {
                                Some(_) => {
                                    println!("Duplicate symbol: {}", *name);
                                    return;
                                },
                                None => { 
                                    symbol_addresses.insert(String::from(*name), exec_offset + ss.address);
                                    if *name == ENTRY {
                                        main_last_instruction_offset = Some(exec_offset + ss.address + ss.size- 2);
                                    }
                                }
                            }
                        },
                        None => ()
                    }
                },
                None => ()
            }

            simple_symbols.push(ss);
        }

        let relocations = match file.section_data_by_name(".rels.cs") {
            Some(v) => v,
            None => {
                println!("Couldn't find relocation section");
                return;
            }
        };

        let code = match file.section_data_by_name(".cs") {
            Some(v) => v,
            None => {
                println!("Couldn't find relocation section");
                return;
            }
        };

        executable.extend(code.iter());

        // Hack to allow the emulator to HLT when main returns by replacing
        // its last instruction with HLT
        match main_last_instruction_offset {
            Some(offset) => {
                let mut v = Vec::new();
                v.push(HLT);
                v.push(HLT);
                executable.splice(offset..(offset + 2), v.iter().cloned());
            },
            None => ()
        }

        let mut rdr = Cursor::new(&relocations);

        while (rdr.position() as usize) < relocations.len() {
            let cs_offset = rdr.read_u32::<LittleEndian>().unwrap() as usize;
            let _typ = rdr.read_u8().unwrap();
            let symbol_id = rdr.read_u8().unwrap() as usize;
            let _pad = rdr.read_u16::<LittleEndian>().unwrap();
            write_what_where.push(WriteSymbol {
                cs_offset: exec_offset + cs_offset,
                symbol_index: symbol_offset + symbol_id
            });
        }
    }

    // Insert the jump to main
    let entry_address = match symbol_addresses.get(ENTRY) {
        Some(address) => ((*address + ENTRYPOINT_LEN) / INSTRUCTION_BYTES).wrapping_sub(1),
        None => {
            println!("Missing entry function: {}", ENTRY);
            return;
        }
    };

    let entrypoint_jmp_code = gen_entrypoint(entry_address);
    assert!(entrypoint_jmp_code.len() == ENTRYPOINT_LEN);

    for www in &write_what_where {
        let i = www.symbol_index;
        let s = &simple_symbols[i];
        let name = s.name.as_ref().unwrap();
        let symbol_address = match symbol_addresses.get(name) {
            Some(address) => ((*address + entrypoint_jmp_code.len()) / INSTRUCTION_BYTES).wrapping_sub(1),
            None => {
                println!("Missing symbol: {}", name);
                return;
            }
        };

        let insert_code = load_addr_code(symbol_address);
        executable.splice(www.cs_offset..(www.cs_offset + insert_code.len()), insert_code.iter().cloned());
    }

    let mut outbuf = fs::File::create("/tmp/henlo.bin").unwrap();
    outbuf.write_all(&entrypoint_jmp_code).unwrap();
    outbuf.write_all(&executable).unwrap();
}
