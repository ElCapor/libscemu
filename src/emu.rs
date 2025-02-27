#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_must_use)]
#![allow(clippy::assertions_on_constants)]

pub mod banzai;
pub mod breakpoint;
pub mod colors;
pub mod console;
pub mod constants;
pub mod context32;
pub mod context64;
pub mod eflags;
pub mod err;
//pub mod endpoint;
mod elf32;
mod elf64;
mod exception;
pub mod flags;
pub mod fpu;
pub mod hook;
mod inline;
pub mod maps;
mod pe32;
mod pe64;
mod peb32;
mod peb64;
pub mod regs64;
pub mod script;
pub mod structures;
pub mod syscall32;
pub mod syscall64;
mod winapi32;
mod winapi64;
mod ntapi32;

use crate::config::Config;
use atty::Stream;
use banzai::Banzai;
use breakpoint::Breakpoint;
use colors::Colors;
use console::Console;
use csv::ReaderBuilder;
use eflags::Eflags;
use elf32::Elf32;
use elf64::Elf64;
use err::ScemuError;
use flags::Flags;
use fpu::FPU;
use hook::Hook;
use maps::Maps;
use pe32::PE32;
use pe64::PE64;
use regs64::Regs64;
use std::collections::BTreeMap;
use std::sync::atomic;
use std::sync::Arc;
use std::time::Instant;
//use std::arch::asm;

use iced_x86::{
    Decoder, DecoderOptions, Formatter, Instruction, InstructionInfoFactory, IntelFormatter,
    MemorySize, Mnemonic, OpKind, Register,
};

/*
macro_rules! rotate_left {
    ($val:expr, $rot:expr, $bits:expr) => {
       ($val << $rot) | ($val >> ($bits-$rot))
    };
}

macro_rules! rotate_right {
    ($val:expr, $rot:expr, $bits:expr) => {
        ($val >> $rot) | ($val << ($bits-$rot))
    };
}*/

macro_rules! get_bit {
    ($val:expr, $count:expr) => {
        ($val & (1 << $count)) >> $count
    };
}

macro_rules! set_bit {
    ($val:expr, $count:expr, $bit:expr) => {
        if $bit == 1 {
            $val |= 1 << $count;
        } else {
            $val &= !(1 << $count);
        }
    };
}

macro_rules! to32 {
    ($val:expr) => {
        ($val & 0xffffffff) as u32
    };
}

pub struct Emu {
    pub regs: Regs64,
    pub pre_op_regs: Regs64,
    pub post_op_regs: Regs64,
    pub flags: Flags,
    pub pre_op_flags: Flags,
    pub post_op_flags: Flags,
    pub eflags: Eflags,
    pub fpu: FPU,
    pub maps: Maps,
    pub hook: Hook,
    exp: u64,
    break_on_alert: bool,
    pub bp: Breakpoint,
    pub seh: u64,
    pub veh: u64,
    pub feh: u64,
    eh_ctx: u32,
    pub cfg: Config,
    colors: Colors,
    pub pos: u64,
    force_break: bool,
    force_reload: bool,
    pub tls_callbacks: Vec<u64>,
    pub tls: Vec<u32>,
    pub fls: Vec<u32>,
    pub out: String,
    main_thread_cont: u64,
    gateway_return: u64,
    is_running: Arc<atomic::AtomicU32>,
    break_on_next_cmp: bool,
    break_on_next_return: bool,
    filename: String,
    enabled_ctrlc: bool,
    run_until_ret: bool,
    running_script: bool,
    banzai: Banzai,
    mnemonic: String,
    dbg: bool,
    linux: bool,
    fs: BTreeMap<u64, u64>,
    now: Instant,
    pub skip_apicall: bool,
    pub its_apicall: Option<u64>,
}

impl Emu {
    pub fn new() -> Emu {
        Emu {
            regs: Regs64::new(),
            pre_op_regs: Regs64::new(),
            post_op_regs: Regs64::new(),
            flags: Flags::new(),
            pre_op_flags: Flags::new(),
            post_op_flags: Flags::new(),
            eflags: Eflags::new(),
            fpu: FPU::new(),
            maps: Maps::new(),
            hook: Hook::new(),
            exp: 0,
            break_on_alert: false,
            bp: Breakpoint::new(),
            seh: 0,
            veh: 0,
            feh: 0,
            eh_ctx: 0,
            cfg: Config::new(),
            colors: Colors::new(),
            pos: 0,
            force_break: false,
            force_reload: false,
            tls_callbacks: Vec::new(),
            tls: Vec::new(),
            fls: Vec::new(),
            out: String::new(),
            main_thread_cont: 0,
            gateway_return: 0,
            is_running: Arc::new(atomic::AtomicU32::new(0)),
            break_on_next_cmp: false,
            break_on_next_return: false,
            filename: String::new(),
            enabled_ctrlc: true,
            run_until_ret: false,
            running_script: false,
            banzai: Banzai::new(),
            mnemonic: String::new(),
            dbg: false,
            linux: false,
            fs: BTreeMap::new(),
            now: Instant::now(),
            skip_apicall: false,
            its_apicall: None,
        }
    }

    pub fn set_base_address(&mut self, addr: u64) {
        self.cfg.code_base_addr = addr;
    }

    pub fn enable_debug_mode(&mut self) {
        self.dbg = true;
    }

    pub fn disable_debug_mode(&mut self) {
        self.dbg = false;
    }

    // configure the base address of stack map
    pub fn set_stack_address(&mut self, addr: u64) {
        self.cfg.stack_addr = addr;
    }

    // select the folder with maps32 or maps64 depending the arch, make sure to do init after this.
    pub fn set_maps_folder(&mut self, folder: &str) {
        let mut f = folder.to_string();
        f.push_str("/");
        self.cfg.maps_folder = folder.to_string();
    }

    // spawn a console on the instruction number, ie: 1 at the beginning.
    pub fn spawn_console_at(&mut self, exp: u64) {
        self.exp = exp;
    }

    pub fn spawn_console_at_addr(&mut self, addr: u64) {
        self.cfg.console2 = true;
        self.cfg.console_addr = addr;
        self.cfg.console_enabled = true;
    }

    pub fn get_base_addr(&self) -> Option<u64> {
        let map = match self.maps.get_map_by_name("code") {
            Some(m) => m,
            None => return None,
        };

        Some(map.get_base())
    }

    pub fn enable_ctrlc(&mut self) {
        self.enabled_ctrlc = true;
    }

    pub fn disable_ctrlc(&mut self) {
        self.enabled_ctrlc = false;
    }

    pub fn disable_console(&mut self) {
        self.cfg.console_enabled = false;
    }

    pub fn enable_console(&mut self) {
        self.cfg.console_enabled = true;
    }

    pub fn set_verbose(&mut self, n: u32) {
        self.cfg.verbose = n;
    }

    pub fn enable_banzai(&mut self) {
        self.cfg.skip_unimplemented = true;
    }

    pub fn disable_banzai(&mut self) {
        self.cfg.skip_unimplemented = false;
    }

    pub fn banzai_add(&mut self, name: &str, nparams: i32) {
        self.banzai.add(name, nparams);
    }

    pub fn api_addr_to_name(&mut self, addr: u64) -> String {
        let name: String;
        if self.cfg.is_64bits {
            name = winapi64::kernel32::resolve_api_addr_to_name(self, addr);
        } else {
            name = winapi32::kernel32::resolve_api_addr_to_name(self, addr);
        }

        return name;
    }

    pub fn api_name_to_addr(&mut self, kw: &str) -> u64 {
        if self.cfg.is_64bits {
            let (addr, lib, name) = winapi64::kernel32::search_api_name(self, &kw);
            return addr;
        } else {
            let (addr, lib, name) = winapi32::kernel32::search_api_name(self, &kw);
            return addr;
        }
    }

    pub fn init_stack32(&mut self) {
        let stack = self.maps.get_mem("stack");

        if self.cfg.stack_addr == 0 {
            self.cfg.stack_addr = 0x212000;
        }

        stack.set_base(self.cfg.stack_addr);
        stack.set_size(0x030000);
        self.regs.set_esp(self.cfg.stack_addr + 0x1c000 + 4);
        self.regs
            .set_ebp(self.cfg.stack_addr + 0x1c000 + 4 + 0x1000);

        assert!(self.regs.get_esp() < self.regs.get_ebp());
        assert!(self.regs.get_esp() > stack.get_base());
        assert!(self.regs.get_esp() < stack.get_bottom());
        assert!(self.regs.get_ebp() > stack.get_base());
        assert!(self.regs.get_ebp() < stack.get_bottom());
        assert!(stack.inside(self.regs.get_esp()));
        assert!(stack.inside(self.regs.get_ebp()));
    }

    pub fn init_stack64(&mut self) {
        let stack = self.maps.get_mem("stack");

        if self.cfg.stack_addr == 0 {
            self.cfg.stack_addr = 0x22a000;
        }

        self.regs.rsp = self.cfg.stack_addr + 0x4000;
        self.regs.rbp = self.cfg.stack_addr + 0x4000 + 0x1000;
        stack.set_base(self.cfg.stack_addr);
        stack.set_size(0x6000);

        assert!(self.regs.rsp < self.regs.rbp);
        assert!(self.regs.rsp > stack.get_base());
        assert!(self.regs.rsp < stack.get_bottom());
        assert!(self.regs.rbp > stack.get_base());
        assert!(self.regs.rbp < stack.get_bottom());
        assert!(stack.inside(self.regs.rsp));
        assert!(stack.inside(self.regs.rbp));
    }

    pub fn init_stack64_tests(&mut self) {
        let stack = self.maps.get_mem("stack");
        self.regs.rsp = 0x000000000014F4B0;
        self.regs.rbp = 0x0000000000000000;
        stack.set_base(0x0000000000149000);
        stack.set_size(0x0000000000007000);
    }

    pub fn init_regs_tests(&mut self) {
        self.regs.rax = 0x00000001448A76A4;
        self.regs.rbx = 0x000000007FFE0385;
        self.regs.rcx = 0x0000000140000000;
        self.regs.rdx = 0x0000000000000001;
        self.regs.rsi = 0x0000000000000001;
        self.regs.rdi = 0x000000007FFE0384;
        self.regs.r10 = 0x000000007FFE0384;
        self.regs.r11 = 0x0000000000000246;
        self.regs.r12 = 0x00000001448A76A4;
        self.regs.r14 = 0x0000000140000000;
    }

    pub fn init_flags_tests(&mut self) {
        self.flags.clear();

        self.flags.f_zf = true;
        self.flags.f_pf = true;
        self.flags.f_af = false;

        self.flags.f_of = false;
        self.flags.f_sf = false;
        self.flags.f_df = false;

        self.flags.f_cf = false;
        self.flags.f_tf = false;
        self.flags.f_if = true;

        self.flags.f_nt = false;
    }

    pub fn init(&mut self) {
        self.pos = 0;

        if !atty::is(Stream::Stdout) {
            self.cfg.nocolors = true;
            self.colors.disable();
            self.cfg.console_enabled = false;
            self.disable_ctrlc();
        }

        //println!("initializing regs");
        self.regs.clear::<64>();
        //self.regs.rand();

        if self.cfg.is_64bits {
            self.regs.rip = self.cfg.entry_point;
            self.maps.is_64bits = true;
            self.init_regs_tests();
            self.init_mem64();
            //self.init_stack64();
            self.init_stack64_tests();
            //self.init_flags_tests();
        } else {
            // 32bits
            self.regs.sanitize32();
            self.regs.set_eip(self.cfg.entry_point);
            self.init_mem32();
            self.init_stack32();
        }

        // loading banzai on 32bits
        if !self.cfg.is_64bits {
            let mut rdr = ReaderBuilder::new()
                .from_path(&format!("{}/banzai.csv", self.cfg.maps_folder))
                .expect("banzai.csv not found on maps folder, please download last scemu maps");

            for result in rdr.records() {
                let record = result.expect("error parsing banzai.csv");
                let api = &record[0];
                let params: i32 = record[1].parse().expect("error parsing maps32/banzai.csv");

                self.banzai.add(api, params);
            }
        }
        //self.init_tests();
    }

    pub fn init_linux64(&mut self, dyn_link: bool) {
        self.regs.clear::<64>();
        self.flags.clear();
        self.flags.f_if = true;

        let orig_path = std::env::current_dir().unwrap();
        std::env::set_current_dir(self.cfg.maps_folder.clone());
        if dyn_link {
            //self.regs.rsp = 0x7fffffffe2b0;
            self.regs.rsp = 0x7fffffffe790;
            self.maps
                .create_map("linux_dynamic_stack")
                .load_at(0x7ffffffde000);
            //self.maps.create_map("dso_dyn").load_at(0x7ffff7ffd0000);
            self.maps.create_map("dso_dyn").load_at(0x7ffff7fd0000);
            self.maps.create_map("linker").load_at(0x7ffff7ffe000);
        } else {
            self.regs.rsp = 0x7fffffffe270;
            self.maps
                .create_map("linux_static_stack")
                .load_at(0x7ffffffde000);
            self.maps.create_map("dso").load_at(0x7ffff7ffd000);
        }
        let tls = self.maps.create_map("tls");
        tls.set_base(0x7ffff7fff000);
        tls.set_size(0xfff);

        std::env::set_current_dir(orig_path);

        if dyn_link {
            //heap.set_base(0x555555579000);
        } else {
            //heap.set_base(0x4b5000);
            let heap = self.maps.create_map("heap");
            heap.set_base(0x4b5b00);
            heap.set_size(0x4d8000 - 0x4b5000);
        }

        self.regs.rbp = 0;

        self.fs.insert(0xffffffffffffffC8, 0); //0x4b6c50
        self.fs.insert(0xffffffffffffffD0, 0);
        self.fs.insert(0xffffffffffffffd8, 0x4b27a0);
        self.fs.insert(0xffffffffffffffa0, 0x4b3980);
        self.fs.insert(0x18, 0);
        self.fs.insert(40, 0x4b27a0);
    }

    pub fn init_mem32(&mut self) {
        //println!("loading memory maps");
        self.maps.create_map("10000");
        self.maps.create_map("20000");
        self.maps.create_map("stack");
        self.maps.create_map("code");
        self.maps.create_map("peb");
        self.maps.create_map("teb");
        self.maps.create_map("ntdll");
        self.maps.create_map("ntdll_text");
        self.maps.create_map("ntdll_data");
        self.maps.create_map("kernel32");
        self.maps.create_map("kernel32_text");
        self.maps.create_map("kernel32_data");
        self.maps.create_map("kernelbase");
        self.maps.create_map("kernelbase_text");
        self.maps.create_map("kernelbase_data");
        self.maps.create_map("msvcrt");
        self.maps.create_map("msvcrt_text");
        self.maps.create_map("reserved");
        self.maps.create_map("kuser_shared_data");
        self.maps.create_map("binary");
        //self.maps.create_map("reserved2");
        self.maps.create_map("ws2_32");
        self.maps.create_map("ws2_32_text");
        self.maps.create_map("wininet");
        self.maps.create_map("wininet_text");
        self.maps.create_map("shlwapi");
        self.maps.create_map("shlwapi_text");
        self.maps.create_map("gdi32");
        self.maps.create_map("gdi32_text");
        self.maps.create_map("user32");
        self.maps.create_map("user32_text");
        self.maps.create_map("lpk");
        self.maps.create_map("lpk_text");
        self.maps.create_map("usp10");
        self.maps.create_map("usp10_text");
        self.maps.create_map("advapi32");
        self.maps.create_map("advapi32_text");
        self.maps.create_map("sechost");
        self.maps.create_map("sechost_text");
        self.maps.create_map("rpcrt4");
        self.maps.create_map("rpcrt4_text");
        self.maps.create_map("urlmon");
        self.maps.create_map("urlmon_text");
        self.maps.create_map("ole32");
        self.maps.create_map("ole32_text");
        self.maps.create_map("oleaut32");
        self.maps.create_map("oleaut32_text");
        self.maps.create_map("crypt32");
        self.maps.create_map("crypt32_text");
        self.maps.create_map("msasn1");
        self.maps.create_map("msasn1_text");
        self.maps.create_map("iertutils");
        self.maps.create_map("iertutils_text");
        self.maps.create_map("imm32");
        self.maps.create_map("imm32_text");
        self.maps.create_map("msctf");
        self.maps.create_map("msctf_text");

        //self.maps.write_byte(0x2c3000, 0x61); // metasploit trick

        let orig_path = std::env::current_dir().unwrap();
        std::env::set_current_dir(self.cfg.maps_folder.clone());

        self.maps.get_mem("code").set_base(self.cfg.code_base_addr);
        let kernel32 = self.maps.get_mem("kernel32");
        kernel32.set_base(0x75e40000);
        if !kernel32.load("kernel32.bin") {
            println!("cannot find the maps files, use --maps flag to speficy the folder.");
            std::process::exit(1);
        }

        let kernel32_text = self.maps.get_mem("kernel32_text");
        kernel32_text.set_base(0x75e41000);
        kernel32_text.load("kernel32_text.bin");

        let kernel32_data = self.maps.get_mem("kernel32_data");
        kernel32_data.set_base(0x75f06000);
        kernel32_data.load("kernel32_data.bin");

        let kernelbase = self.maps.get_mem("kernelbase");
        kernelbase.set_base(0x75940000);
        kernelbase.load("kernelbase.bin");

        let kernelbase_text = self.maps.get_mem("kernelbase_text");
        kernelbase_text.set_base(0x75941000);
        kernelbase_text.load("kernelbase_text.bin");

        let kernelbase_data = self.maps.get_mem("kernelbase_data");
        kernelbase_data.set_base(0x75984000);
        kernelbase_data.load("kernelbase_data.bin");

        let msvcrt = self.maps.get_mem("msvcrt");
        msvcrt.set_base(0x761e0000);
        msvcrt.load("msvcrt.bin");

        let msvcrt_text = self.maps.get_mem("msvcrt_text");
        msvcrt_text.set_base(0x761e1000);
        msvcrt_text.load("msvcrt_text.bin");

        /*let reserved2 = self.maps.get_mem("reserved2");
        reserved2.set_base(0x2c3000); //0x2c3018
        reserved2.set_size(0xfd000);*/

        let reserved = self.maps.get_mem("reserved");
        reserved.set_base(0x2c0000);
        reserved.load("reserved.bin");
        assert!(reserved.read_byte(0x2c31a0) != 0);

        let peb = self.maps.get_mem("peb");
        peb.set_base(0x7ffdf000);
        peb.load("peb.bin");

        let peb = self.maps.get_mem("peb");
        peb.write_byte(peb.get_base() + 2, 0);

        //let peb = peb32::init_peb(self, space_addr, base);
        //self.maps.write_dword(peb + 8, base);


        let teb = self.maps.get_mem("teb");
        teb.set_base(0x7ffde000);
        teb.load("teb.bin");

        let ntdll = self.maps.get_mem("ntdll");
        ntdll.set_base(0x77570000);
        ntdll.load("ntdll.bin");

        let ntdll_text = self.maps.get_mem("ntdll_text");
        ntdll_text.set_base(0x77571000);
        ntdll_text.load("ntdll_text.bin");

        let ntdll_data = self.maps.get_mem("ntdll_data");
        ntdll_data.set_base(0x77647000);
        ntdll_data.load("ntdll_data.bin");

        let kuser_shared_data = self.maps.get_mem("kuser_shared_data");
        kuser_shared_data.set_base(0x7ffe0000);
        kuser_shared_data.load("kuser_shared_data.bin");

        let binary = self.maps.get_mem("binary");
        binary.set_base(0x400000);
        binary.set_size(0x1000);

        let ws2_32 = self.maps.get_mem("ws2_32");
        ws2_32.set_base(0x77480000);
        ws2_32.load("ws2_32.bin");

        let ws2_32_text = self.maps.get_mem("ws2_32_text");
        ws2_32_text.set_base(0x77481000);
        ws2_32_text.load("ws2_32_text.bin");

        let wininet = self.maps.get_mem("wininet");
        wininet.set_base(0x76310000);
        wininet.load("wininet.bin");

        let wininet_text = self.maps.get_mem("wininet_text");
        wininet_text.set_base(0x76311000);
        wininet_text.load("wininet_text.bin");

        let shlwapi = self.maps.get_mem("shlwapi");
        shlwapi.set_base(0x76700000);
        shlwapi.load("shlwapi.bin");

        let shlwapi_text = self.maps.get_mem("shlwapi_text");
        shlwapi_text.set_base(0x76701000);
        shlwapi_text.load("shlwapi_text.bin");

        let gdi32 = self.maps.get_mem("gdi32");
        gdi32.set_base(0x759c0000);
        gdi32.load("gdi32.bin");

        let gdi32_text = self.maps.get_mem("gdi32_text");
        gdi32_text.set_base(0x759c1000);
        gdi32_text.load("gdi32_text.bin");

        let user32 = self.maps.get_mem("user32");
        user32.set_base(0x773b0000);
        user32.load("user32.bin");

        let user32_text = self.maps.get_mem("user32_text");
        user32_text.set_base(0x773b1000);
        user32_text.load("user32_text.bin");

        let lpk = self.maps.get_mem("lpk");
        lpk.set_base(0x75b00000);
        lpk.load("lpk.bin");

        let lpk_text = self.maps.get_mem("lpk_text");
        lpk_text.set_base(0x75b01000);
        lpk_text.load("lpk_text.bin");

        let usp10 = self.maps.get_mem("usp10");
        usp10.set_base(0x76660000);
        usp10.load("usp10.bin");

        let usp10_text = self.maps.get_mem("usp10_text");
        usp10_text.set_base(0x76661000);
        usp10_text.load("usp10_text.bin");

        let advapi32 = self.maps.get_mem("advapi32");
        advapi32.set_base(0x776f0000);
        advapi32.load("advapi32.bin");

        let advapi32_text = self.maps.get_mem("advapi32_text");
        advapi32_text.set_base(0x776f1000);
        advapi32_text.load("advapi32_text.bin");

        let sechost = self.maps.get_mem("sechost");
        sechost.set_base(0x75a10000);
        sechost.load("sechost.bin");

        let sechost_text = self.maps.get_mem("sechost_text");
        sechost_text.set_base(0x75a11000);
        sechost_text.load("sechost_text.bin");

        let rpcrt4 = self.maps.get_mem("rpcrt4");
        rpcrt4.set_base(0x774c0000);
        rpcrt4.load("rpcrt4.bin");

        let rpcrt4_text = self.maps.get_mem("rpcrt4_text");
        rpcrt4_text.set_base(0x774c1000);
        rpcrt4_text.load("rpcrt4_text.bin");

        let urlmon = self.maps.get_mem("urlmon");
        urlmon.set_base(0x75b60000);
        urlmon.load("urlmon.bin");

        let urlmon_text = self.maps.get_mem("urlmon_text");
        urlmon_text.set_base(0x75b61000);
        urlmon_text.load("urlmon_text.bin");

        let ole32 = self.maps.get_mem("ole32");
        ole32.set_base(0x76500000);
        ole32.load("ole32.bin");

        let ole32_text = self.maps.get_mem("ole32_text");
        ole32_text.set_base(0x76501000);
        ole32_text.load("ole32_text.bin");

        let oleaut32 = self.maps.get_mem("oleaut32");
        oleaut32.set_base(0x76470000);
        oleaut32.load("oleaut32.bin");

        let oleaut32_text = self.maps.get_mem("oleaut32_text");
        oleaut32_text.set_base(0x76471000);
        oleaut32_text.load("oleaut32_text.bin");

        let crypt32 = self.maps.get_mem("crypt32");
        crypt32.set_base(0x757d0000);
        crypt32.load("crypt32.bin");

        let crypt32_text = self.maps.get_mem("crypt32_text");
        crypt32_text.set_base(0x757d1000);
        crypt32_text.load("crypt32_text.bin");

        let msasn1 = self.maps.get_mem("msasn1");
        msasn1.set_base(0x75730000);
        msasn1.load("msasn1.bin");

        let msasn1_text = self.maps.get_mem("msasn1_text");
        msasn1_text.set_base(0x75731000);
        msasn1_text.load("msasn1_text.bin");

        let iertutils = self.maps.get_mem("iertutils");
        iertutils.set_base(0x75fb0000);
        iertutils.load("iertutils.bin");

        let iertutils_text = self.maps.get_mem("iertutils_text");
        iertutils_text.set_base(0x75fb1000);
        iertutils_text.load("iertutils_text.bin");

        let imm32 = self.maps.get_mem("imm32");
        imm32.set_base(0x776d0000);
        imm32.load("imm32.bin");

        let imm32_text = self.maps.get_mem("imm32_text");
        imm32_text.set_base(0x776d1000);
        imm32_text.load("imm32_text.bin");

        let msctf = self.maps.get_mem("msctf");
        msctf.set_base(0x75a30000);
        msctf.load("msctf.bin");

        let msctf_text = self.maps.get_mem("msctf_text");
        msctf_text.set_base(0x75a31000);
        msctf_text.load("msctf_text.bin");

        let m10000 = self.maps.get_mem("10000");
        m10000.set_base(0x10000);
        m10000.load("m10000.bin");
        m10000.set_size(0xffff);

        let m20000 = self.maps.get_mem("20000");
        m20000.set_base(0x20000);
        m20000.load("m20000.bin");
        m20000.set_size(0xffff);

        let (base, pe_hdr) = self.load_pe32("nsi.dll", false, 0x776c0000);

        //let nsi = self.maps.create_map("NSI.dll");
        //nsi.set_base(0x776c0000);
        //nsi.load("nsi.dll");

        // xloader initial state hack
        //self.memory_write("dword ptr [esp + 4]", 0x22a00);
        //self.maps.get_mem("kernel32_xloader").set_base(0x75e40000)

        std::env::set_current_dir(orig_path);
    }

    pub fn init_tests(&mut self) {
        let mem = self.maps.create_map("test");
        mem.set_base(0);
        mem.set_size(1024);
        mem.write_qword(0, 0x1122334455667788);
        assert!(mem.read_qword(0) == 0x1122334455667788);
        self.maps.free("test");

        // some tests
        assert!(get_bit!(0xffffff00u32, 0) == 0);
        assert!(get_bit!(0xffffffffu32, 5) == 1);
        assert!(get_bit!(0xffffff00u32, 5) == 0);
        assert!(get_bit!(0xffffff00u32, 7) == 0);
        assert!(get_bit!(0xffffff00u32, 8) == 1);

        let mut a: u32 = 0xffffff00;
        set_bit!(a, 0, 1);
        set_bit!(a, 1, 1);
        set_bit!(a, 2, 1);
        set_bit!(a, 3, 1);
        set_bit!(a, 4, 1);
        set_bit!(a, 5, 1);
        set_bit!(a, 6, 1);
        set_bit!(a, 7, 1);

        assert!(a == 0xffffffff);

        set_bit!(a, 0, 0);
        set_bit!(a, 1, 0);
        set_bit!(a, 2, 0);
        set_bit!(a, 3, 0);
        set_bit!(a, 4, 0);
        set_bit!(a, 5, 0);
        set_bit!(a, 6, 0);
        set_bit!(a, 7, 0);

        assert!(a == 0xffffff00);

        let mut r: u64;
        (r, _) = self.shrd(0x9fd88893, 0x1b, 0x6, 32);
        assert!(r == 0x6e7f6222);
        (r, _) = self.shrd(0x6fdcb03, 0x0, 0x6, 32);
        assert!(r == 0x1bf72c);
        (r, _) = self.shrd(0x91545f1d, 0x6fe2, 0x6, 32);
        assert!(r == 0x8a45517c);
        (r, _) = self.shld(0x1b, 0xf1a7eb1d, 0xa, 32);
        assert!(r == 0x6fc6);
        (r, _) = self.shld(0x1, 0xffffffff, 4, 32);
        assert!(r == 0x1f);
        (r, _) = self.shld(0x1, 0xffffffff, 33, 32);
        assert!(r == 0x3);
        (r, _) = self.shld(0x144e471f8, 0x14F498, 0x3e, 64);
        assert!(r == 0x53d26);

        if self.maps.mem_test() {
            println!("memory test Ok.");
        } else {
            eprintln!("It doesn't pass the memory tests!!");
            self.spawn_console();
            std::process::exit(1);
        }
    }

    pub fn init_mem64(&mut self) {
        println!("loading memory maps");

        let orig_path = std::env::current_dir().unwrap();
        std::env::set_current_dir(self.cfg.maps_folder.clone());

        self.maps.create_map("m10000").load_at(0x10000);
        self.maps.create_map("m20000").load_at(0x20000);
        self.maps.create_map("m520000").load_at(0x520000);
        self.maps.create_map("m53b000").load_at(0x53b000);
        //self.maps.create_map("exe_pe").load_at(0x400000);
        //self.maps.create_map("calc").load_at(0x400000);
        self.maps.create_map("calc").load_at(0x110000000);
        self.maps
            .create_map("code")
            .set_base(self.cfg.code_base_addr);
        self.maps.create_map("stack");
        //self.maps.create_map("peb").load_at(0x7fffffdf000);
        peb64::init_peb(self);
        self.maps.create_map("teb").load_at(0x7fffffdd000);
        self.maps.create_map("ntdll_pe").load_at(0x76fd0000);
        self.maps.create_map("ntdll_text").load_at(0x76fd1000);
        self.maps.create_map("ntdll_rt").load_at(0x770d2000);
        self.maps.create_map("ntdll_rdata").load_at(0x770d3000);
        self.maps.create_map("ntdll_data").load_at(0x77102000);
        self.maps.create_map("kernel32_pe").load_at(0x76db0000);
        self.maps.create_map("kernel32_text").load_at(0x76db1000);
        self.maps.create_map("kernel32_rdata").load_at(0x76e4c000);
        self.maps.create_map("kernel32_data").load_at(0x76eba000);
        self.maps.create_map("kernelbase_pe").load_at(0x7fefd010000);
        self.maps
            .create_map("kernelbase_text")
            .load_at(0x7fefd011000);
        self.maps
            .create_map("kernelbase_rdata")
            .load_at(0x7fefd05a000);
        self.maps
            .create_map("kernelbase_data")
            .load_at(0x7fefd070000);
        self.maps.create_map("msvcrt_pe").load_at(0x7fefef00000);
        self.maps.create_map("msvcrt_text").load_at(0x7fefef01000);
        self.maps.create_map("msvcrt_rdata").load_at(0x7fefef7a000);
        self.maps.create_map("user32_pe").load_at(0x76ed0000);
        self.maps.create_map("user32_text").load_at(0x76ed1000);
        self.maps.create_map("user32_rdata").load_at(0x76f52000);
        self.maps.create_map("msasn1_pe").load_at(0x7fefcfc0000);
        self.maps.create_map("msasn1_text").load_at(0x7fefcfc1000);
        self.maps.create_map("msasn1_rdata").load_at(0x7fefcfc9000);
        self.maps.create_map("crypt32_pe").load_at(0x7fefd0c0000);
        self.maps.create_map("crypt32_text").load_at(0x7fefd0c1000);
        self.maps.create_map("crypt32_rdata").load_at(0x7fefd18f000);
        self.maps.create_map("msctf_pe").load_at(0x7fefd2f0000);
        self.maps.create_map("msctf_text").load_at(0x7fefd2f1000);
        self.maps.create_map("msctf_rdata").load_at(0x7fefd391000);
        self.maps.create_map("iertutil_pe").load_at(0x7fefd400000);
        self.maps.create_map("iertutil_text").load_at(0x7fefd401000);
        self.maps
            .create_map("iertutil_rdata")
            .load_at(0x7fefd43e000);
        self.maps.create_map("ole32_pe").load_at(0x7fefd660000);
        self.maps.create_map("ole32_text").load_at(0x7fefd661000);
        self.maps.create_map("ole32_rdata").load_at(0x7fefd7df000);
        self.maps.create_map("lpk_pe").load_at(0x7fefd870000);
        self.maps.create_map("lpk_text").load_at(0x7fefd871000);
        self.maps.create_map("lpk_rdata").load_at(0x7fefd878000);
        self.maps.create_map("wininet_pe").load_at(0x7fefd880000);
        self.maps.create_map("wininet_text").load_at(0x7fefd881000);
        self.maps.create_map("wininet_rdata").load_at(0x7fefd940000);
        self.maps.create_map("gdi32_pe").load_at(0x7fefd9b0000);
        self.maps.create_map("gdi32_text").load_at(0x7fefd9b1000);
        self.maps.create_map("gdi32_rdata").load_at(0x7fefda02000);
        self.maps.create_map("imm32_pe").load_at(0x7fefe990000);
        self.maps.create_map("imm32_text").load_at(0x7fefe991000);
        self.maps.create_map("imm32_rdata").load_at(0x7fefe9ad000);
        self.maps.create_map("usp10_pe").load_at(0x7fefe9c0000);
        self.maps.create_map("usp10_text").load_at(0x7fefe9c1000);
        self.maps.create_map("sechost_pe").load_at(0x7fefea90000);
        self.maps.create_map("sechost_text").load_at(0x7fefea91000);
        self.maps.create_map("rpcrt4_pe").load_at(0x7fefeab0000);
        self.maps.create_map("rpcrt4_text").load_at(0x7fefeab1000);
        self.maps.create_map("rpcrt4_rdata").load_at(0x7fefeb93000);
        self.maps.create_map("nsi_pe").load_at(0x7fefebe0000);
        self.maps.create_map("nsi_text").load_at(0x7fefebe1000);
        self.maps.create_map("nsi_rdata").load_at(0x7fefebe3000);
        self.maps.create_map("urlmon_pe").load_at(0x7fefed30000);
        self.maps.create_map("urlmon_text").load_at(0x7fefed31000);
        self.maps.create_map("urlmon_rdata").load_at(0x7fefee05000);
        self.maps.create_map("ws2_32_pe").load_at(0x7fefeeb0000);
        self.maps.create_map("ws2_32_text").load_at(0x7fefeeb1000);
        self.maps.create_map("ws2_32_rdata").load_at(0x7fefeee1000);
        //self.maps.create_map("msvcrt_pe").load_at(0x7fefef00000);
        //self.maps.create_map("msvcrt_text").load_at(0x7fefef01000);
        self.maps.create_map("advapi32_pe").load_at(0x7fefefa0000);
        self.maps.create_map("advapi32_text").load_at(0x7fefefa1000);
        self.maps
            .create_map("advapi32_rdata")
            .load_at(0x7feff00a000);
        self.maps.create_map("oleaut32_pe").load_at(0x7feff180000);
        self.maps.create_map("oleaut32_text").load_at(0x7feff181000);
        self.maps
            .create_map("oleaut32_rdata")
            .load_at(0x7feff21d000);
        self.maps.create_map("shlwapi_pe").load_at(0x7feff260000);
        self.maps.create_map("shlwapi_text").load_at(0x7feff261000);
        self.maps.create_map("shlwapi_rdata").load_at(0x7feff2a5000);

        // load from dll not from maps
        self.maps.create_map("winhttp_pe").load_at(0x7fef9760000);
        self.maps.create_map("winhttp_text").load_at(0x7fef9761000);

        self.maps.create_map("dnsapi_pe").load_at(0x7fefc5f0000);
        self.maps.create_map("dnsapi_text").load_at(0x7fefc5f1000);

        /*self.maps.create_map("iphlpapi_pe").load_at(0x7fefc1b0000);
        self.maps.create_map("iphlpapi_text").load_at(0x7fefc1b1000);*/

        // peb64 patch for being_debugged
        let peb = self.maps.get_mem("peb");
        peb.write_byte(peb.get_base() + 2, 0);

        std::env::set_current_dir(orig_path);

        winapi64::kernel32::load_library(self, "iphlpapi.dll");
        winapi64::kernel32::load_library(self, "winhttp.dll");
        winapi64::kernel32::load_library(self, "dnsapi.dll");
    }

    pub fn filename_to_mapname(&self, filename: &str) -> String {
        let spl: Vec<&str> = filename.split('/').collect();
        let spl2: Vec<&str> = spl[spl.len() - 1].split('.').collect();
        spl2[0].to_string()
    }

    pub fn load_pe32(&mut self, filename: &str, set_entry: bool, force_base: u32) -> (u32, u32) {
        let is_maps = filename.contains("maps32/");
        let mut pe32 = PE32::load(filename);
        let mut base;

        if force_base > 0 {
            base = force_base;
        } else {
            base = pe32.opt.image_base;
        }

        if !is_maps && self.cfg.code_base_addr != 0x3c0000 {
            base = self.cfg.code_base_addr as u32;
        }

        let map_name = self.filename_to_mapname(filename);

        if set_entry {
            let space_addr = peb32::create_ldr_entry(
                self,
                base as u64,
                pe32.dos.e_lfanew,
                &map_name,
                0,
                0x2c1950,
            );
            let peb = peb32::init_peb(self, space_addr, base);
            self.maps.write_dword(peb + 8, base);

            if !is_maps {
                pe32.iat_binding(self);
                pe32.delay_load_binding(self);
            }
        }

        //TODO: query if this vaddr is already used
        let pemap = self.maps.create_map(&format!("{}.pe", map_name));
        pemap.set_base(base.into());
        pemap.set_size(pe32.opt.size_of_headers.into());
        pemap.memcpy(pe32.get_headers(), pe32.opt.size_of_headers as usize);

        /*println!("Loaded {}", filename);
        println!(
            "\t{} sections  base addr 0x{:x}",
            pe32.num_of_sections(),
            base
        );*/

        for i in 0..pe32.num_of_sections() {
            let base: u32;
            if force_base > 0 {
                base = force_base;
            } else {
                if self.cfg.code_base_addr == 0x3c0000 || is_maps {
                    base = pe32.opt.image_base;
                } else {
                    base = self.cfg.code_base_addr as u32;
                }
            }
            let ptr = pe32.get_section_ptr(i);
            let sect = pe32.get_section(i);
            let map;

            if force_base == 0 && sect.get_name() == ".text" && !is_maps {
                map = self.maps.get_map_by_name_mut("code").unwrap();
            } else {
                map = self.maps.create_map(&format!(
                    "{}{}",
                    map_name,
                    sect.get_name()
                        .replace(" ", "")
                        .replace("\t", "")
                        .replace("\x0a", "")
                        .replace("\x0d", "")
                ));
            }

            //println!("-x-> {} {:x}+{:x} = {:x}", sect.get_name(), base, sect.virtual_address, base as u64 + sect.virtual_address as u64);
            map.set_base(base as u64 + sect.virtual_address as u64);
            if sect.virtual_size > sect.size_of_raw_data {
                map.set_size(sect.virtual_size as u64);
            } else {
                map.set_size(sect.size_of_raw_data as u64);
            }
            if sect.get_name() == ".text" {
                //map.set_size( (map.size() + 0x1000) as u64 );
                map.set_size(map.size() as u64);
            }
            map.memcpy(ptr, ptr.len());

            /*println!(
                "\tcreated pe32 map for section `{}` at 0x{:x} size: {}",
                sect.get_name(),
                map.get_base(),
                sect.virtual_size
            );*/
            if set_entry {
                if sect.get_name() == ".text" || i == 0 {
                    if self.cfg.entry_point != 0x3c0000 {
                        self.regs.rip = self.cfg.entry_point;
                        println!(
                            "entry point at 0x{:x} but forcing it at 0x{:x} by -a flag",
                            base as u64 + pe32.opt.address_of_entry_point as u64,
                            self.regs.rip
                        );
                    } else {
                        self.regs.rip = base as u64 + pe32.opt.address_of_entry_point as u64;
                    }
                    println!(
                        "\tentry point at 0x{:x}  0x{:x} ",
                        self.regs.rip, pe32.opt.address_of_entry_point
                    );
                } else if sect.get_name() == ".tls" {
                    let tls_off = sect.pointer_to_raw_data;
                    self.tls_callbacks = pe32.get_tls_callbacks(sect.virtual_address);
                }
            }
        }

        let pe_hdr_off = pe32.dos.e_lfanew;

        pe32.clear();
        return (base, pe_hdr_off);
    }

    pub fn peb64_link(&mut self, libname: &str, base: u64) {
        peb64::dynamic_link_module(base, 0x3c, libname, self);
    }

    pub fn load_pe64(&mut self, filename: &str, set_entry: bool, force_base: u64) -> (u64, u32) {
        let is_maps = filename.contains("maps64/");
        let mut pe64 = PE64::load(filename);
        let base: u64;

        if force_base != 0 {
            if self.maps.overlaps(force_base, pe64.size()) {
                panic!("the forced base address overlaps");
            } else {
                base = force_base;
            }
        } else {
            if set_entry {
                if self.cfg.code_base_addr != 0x3c0000 {
                    base = self.cfg.code_base_addr;
                    if self.maps.overlaps(base, pe64.size()) {
                        panic!("the setted base address overlaps");
                    }
                } else {
                    if self.maps.overlaps(pe64.opt.image_base, pe64.size()) {
                        base = self.maps.alloc(pe64.size()).expect("out of memory");
                    } else {
                        base = pe64.opt.image_base;
                    }
                }
            } else {
                if self.maps.overlaps(pe64.opt.image_base, pe64.size()) {
                    base = self.maps.lib64_alloc(pe64.size()).expect("out of memory");
                } else {
                    if pe64.opt.image_base < constants::LIB64_BARRIER {
                        base = self.maps.lib64_alloc(pe64.size()).expect("out of memory");
                    } else {
                        base = pe64.opt.image_base;
                    }
                }
            }
        }

        /*
        if force_base > 0 {
            base = force_base;
        } else {
            if is_maps && pe64.is_dll() {
                base = self.maps.lib64_alloc(pe64.size()).expect("out of memory");
            } else {
                base = match self.maps.get_mem_by_addr(pe64.opt.image_base + 0x1000) {
                    Some(_) => self.maps.alloc(pe64.size()).expect("out of memory"),
                    None => pe64.opt.image_base,
                };
            }
        }

        if set_entry && !is_maps && self.cfg.code_base_addr != 0x3c0000 {
            base = self.cfg.code_base_addr;
        }
        */

        let map_name = self.filename_to_mapname(filename);

        if set_entry && !is_maps {
            pe64.iat_binding(self);
            pe64.delay_load_binding(self);
        }

        //TODO: query if this vaddr is already used
        let pemap = self.maps.create_map(&format!("{}.pe", map_name));
        pemap.set_base(base.into());
        pemap.set_size(pe64.opt.size_of_headers.into());
        pemap.memcpy(pe64.get_headers(), pe64.opt.size_of_headers as usize);

        /*println!("Loaded {}", filename);
        println!(
            "\t{} sections, base addr 0x{:x}",
            pe64.num_of_sections(),
            base
        );*/

        for i in 0..pe64.num_of_sections() {
            /*let base;
            if force_base > 0 {
                base = force_base;
            } else {
                base = pe64.opt.image_base;
            }*/
            //println!("id:{} name:{}", i, pe64.sect_hdr[i].get_name());
            let ptr = pe64.get_section_ptr(i);
            let sect = pe64.get_section(i);
            let map = self.maps.create_map(&format!(
                "{}{}",
                map_name,
                sect.get_name()
                    .replace(" ", "")
                    .replace("\t", "")
                    .replace("\x0a", "")
                    .replace("\x0d", "")
            ));

            map.set_base(base + sect.virtual_address as u64);
            if sect.virtual_size > sect.size_of_raw_data {
                map.set_size(sect.virtual_size as u64);
            } else {
                map.set_size(sect.size_of_raw_data as u64);
            }
            map.memcpy(ptr, ptr.len());

            /*println!(
                "\tcreated pe64 map for section `{}` at 0x{:x} size: {}",
                sect.get_name(),
                map.get_base(),
                sect.virtual_size
            );*/

            if set_entry {
                if sect.get_name() == ".text" || i == 0 {
                    if pe64.opt.address_of_entry_point == 0 {
                        println!("zero entry!!");
                        self.regs.rip =
                            base + sect.virtual_address as u64 + sect.pointer_to_raw_data as u64;
                    } else {
                        self.regs.rip = base + pe64.opt.address_of_entry_point as u64;
                    }

                    println!(
                        "\tentry point at 0x{:x}  0x{:x} ",
                        self.regs.rip, pe64.opt.address_of_entry_point
                    );
                } else if sect.get_name() == ".tls" {
                    let tls_off = sect.pointer_to_raw_data;
                    self.tls_callbacks = pe64.get_tls_callbacks(sect.virtual_address);
                }
            }
        }

        let pe_hdr_off = pe64.dos.e_lfanew;

        pe64.clear();
        return (base, pe_hdr_off);
    }

    pub fn set_config(&mut self, cfg: Config) {
        self.cfg = cfg;
        if self.cfg.console {
            self.exp = self.cfg.console_num;
        }
        if self.cfg.nocolors {
            self.colors.disable();
        }
    }

    pub fn load_code(&mut self, filename: &str) {
        self.filename = filename.to_string();
        self.cfg.filename = self.filename.clone();

        //let map_name = self.filename_to_mapname(filename);
        //self.cfg.filename = map_name;

        if Elf32::is_elf32(filename) {
            self.linux = true;
            self.cfg.is_64bits = false;

            println!("elf32 detected.");
            let mut elf32 = Elf32::parse(filename).unwrap();
            elf32.load(&mut self.maps);
            self.regs.rip = elf32.elf_hdr.e_entry.into();
            let stack_sz = 0x30000;
            let stack = self.alloc("stack", stack_sz);
            self.regs.rsp = stack + (stack_sz / 2);
            //unimplemented!("elf32 is not supported for now");
        } else if Elf64::is_elf64(filename) {
            self.linux = true;
            self.cfg.is_64bits = true;
            self.maps.clear();

            println!("elf64 detected.");

            let mut elf64 = Elf64::parse(filename).unwrap();
            let dyn_link = elf64.get_dynamic().len() > 0;
            elf64.load(
                &mut self.maps,
                "elf64",
                false,
                dyn_link,
                self.cfg.code_base_addr,
            );
            self.init_linux64(dyn_link);

            if dyn_link {
                let mut ld = Elf64::parse("/lib64/ld-linux-x86-64.so.2").unwrap();
                ld.load(&mut self.maps, "ld-linux", true, dyn_link, 0x3c0000);
                println!("--- emulating ld-linux _start ---");

                self.regs.rip = ld.elf_hdr.e_entry + elf64::LD_BASE;
                self.run(None);
            } else {
                self.regs.rip = elf64.elf_hdr.e_entry;
            }

            /*
            for lib in elf64.get_dynamic() {
                println!("dynamic library {}", lib);
                let libspath = "/usr/lib/x86_64-linux-gnu/";
                let libpath = format!("{}{}", libspath, lib);
                let mut elflib = Elf64::parse(&libpath).unwrap();
                elflib.load(&mut self.maps, &lib, true);

                if lib.contains("libc") {
                    elflib.craft_libc_got(&mut self.maps, "elf64");
                }

                /*
                match elflib.init {
                    Some(addr) => {
                        self.call64(addr, &[]);
                    }
                    None => {}
                }*/
            }*/
        } else if !self.cfg.is_64bits && PE32::is_pe32(filename) {
            println!("PE32 header detected.");
            let (base, pe_off) = self.load_pe32(filename, true, 0);
            let ep = self.regs.rip;

            // emulating tls callbacks
            for i in 0..self.tls_callbacks.len() {
                self.regs.rip = self.tls_callbacks[i];
                println!("emulating tls_callback {} at 0x{:x}", i + 1, self.regs.rip);
                self.stack_push32(base);
                self.run(Some(base as u64));
            }

            self.regs.rip = ep;
        } else if self.cfg.is_64bits && PE64::is_pe64(filename) {
            println!("PE64 header detected.");
            let (base, pe_off) = self.load_pe64(filename, true, 0);
            let ep = self.regs.rip;

            // emulating tls callbacks
            for i in 0..self.tls_callbacks.len() {
                self.regs.rip = self.tls_callbacks[i];
                println!("emulating tls_callback {} at 0x{:x}", i + 1, self.regs.rip);
                self.stack_push64(base);
                self.run(Some(base));
            }

            self.regs.rip = ep;
        } else {
            // shellcode

            println!("shellcode detected.");
            if !self.cfg.is_64bits {
                peb32::init_peb(self, 0x2c18c0, 0);
            }

            if !self.maps.get_mem("code").load(filename) {
                println!("shellcode not found, select the file with -f");
                std::process::exit(1);
            }
            let code = self.maps.get_mem("code");
            code.extend(0xffff);
        }

        if self.cfg.entry_point != 0x3c0000 {
            self.regs.rip = self.cfg.entry_point;
        }

        /*if self.cfg.code_base_addr != 0x3c0000 {
            let code = self.maps.get_mem("code");
            code.update_base(self.cfg.code_base_addr);
            code.update_bottom(self.cfg.code_base_addr + code.size() as u64);
        }*/
    }

    pub fn load_code_bytes(&mut self, bytes: &[u8]) {
        if self.cfg.verbose >= 1 {
            println!("Loading shellcode from bytes");
        }
        if self.cfg.code_base_addr != 0x3c0000 {
            let code = self.maps.get_mem("code");
            code.update_base(self.cfg.code_base_addr);
            code.update_bottom(self.cfg.code_base_addr + code.size() as u64);
        }
        let code = self.maps.get_mem("code");
        let base = code.get_base();
        code.set_size(bytes.len() as u64);
        code.write_bytes(base, bytes);
    }

    pub fn free(&mut self, name: &str) {
        self.maps.free(name);
    }

    pub fn alloc(&mut self, name: &str, size: u64) -> u64 {
        let addr = match self.maps.alloc(size) {
            Some(a) => a,
            None => {
                println!("low memory");
                return 0;
            }
        };
        let map = self.maps.create_map(name);
        map.set_base(addr);
        map.set_size(size);
        addr
    }

    pub fn stack_push32(&mut self, value: u32) -> bool {
        if self.cfg.stack_trace {
            println!("--- stack push32 ---");
            self.maps.dump_dwords(self.regs.get_esp(), 5);
        }

        if self.cfg.trace_mem {
            let name = match self.maps.get_addr_name(self.regs.get_esp()) {
                Some(n) => n,
                None => "not mapped".to_string(),
            };
            println!("\tmem_trace: pos = {} rip = {:x} op = write bits = {} address = 0x{:x} value = 0x{:x} name = '{}'",
                self.pos, self.regs.rip, 32, self.regs.get_esp(), value, name);
        }

        self.regs.set_esp(self.regs.get_esp() - 4);

        /*
        let stack = self.maps.get_mem("stack");
        if stack.inside(self.regs.get_esp()) {
            if !self.maps.write_dword(self.regs.get_esp(), value) {
                //if !stack.write_dword(self.regs.get_esp(), value) {
                return false;
            }
        } else {
            let mem = match self.maps.get_mem_by_addr(self.regs.get_esp()) {
                Some(m) => m,
                None => {
                    println!(
                        "/!\\ pushing stack outside maps esp: 0x{:x}",
                        self.regs.get_esp()
                    );
                    self.spawn_console();
                    return false;
                }
            };
            if !self.maps.write_dword(self.regs.get_esp(), value) {
                //if !mem.write_dword(self.regs.get_esp(), value) {
                return false;
            }
        }*/

        if self.maps.write_dword(self.regs.get_esp(), value) {
            return true;
        } else {
            println!("/!\\ pushing in non mapped mem 0x{:x}", self.regs.get_esp());
            return false;
        }
    }

    pub fn stack_push64(&mut self, value: u64) -> bool {
        if self.cfg.stack_trace {
            println!("--- stack push64  ---");
            self.maps.dump_qwords(self.regs.rsp, 5);
        }

        if self.cfg.trace_mem {
            let name = match self.maps.get_addr_name(self.regs.rsp) {
                Some(n) => n,
                None => "not mapped".to_string(),
            };
            println!("\tmem_trace: pos = {} rip = {:x} op = write bits = {} address = 0x{:x} value = 0x{:x} name = '{}'", self.pos, self.regs.rip, 64, self.regs.rsp, value, name);
        }

        self.regs.rsp -= 8;
        /*
        let stack = self.maps.get_mem("stack");
        if stack.inside(self.regs.rsp) {
            stack.write_qword(self.regs.rsp, value);
        } else {
            let mem = match self.maps.get_mem_by_addr(self.regs.rsp) {
                Some(m) => m,
                None => {
                    println!(
                        "pushing stack outside maps rsp: 0x{:x}",
                        self.regs.get_esp()
                    );
                    self.spawn_console();
                    return false;
                }
            };
            mem.write_qword(self.regs.rsp, value);
        }*/

        if self.maps.write_qword(self.regs.rsp, value) {
            return true;
        } else {
            println!("/!\\ pushing in non mapped mem 0x{:x}", self.regs.rsp);
            return false;
        }
    }

    pub fn stack_pop32(&mut self, pop_instruction: bool) -> Option<u32> {
        if self.cfg.stack_trace {
            println!("--- stack pop32 ---");
            self.maps.dump_dwords(self.regs.get_esp(), 5);
        }

        /*
        let stack = self.maps.get_mem("stack");
        if stack.inside(self.regs.get_esp()) {
            //let value = stack.read_dword(self.regs.get_esp());
            let value = match self.maps.read_dword(self.regs.get_esp()) {
                Some(v) => v,
                None => {
                    println!("esp out of stack");
                    return None;
                }
            };
            if self.cfg.verbose >= 1
                && pop_instruction
                && self.maps.get_mem("code").inside(value.into())
            {
                println!("/!\\ poping a code address 0x{:x}", value);
            }
            self.regs.set_esp(self.regs.get_esp() + 4);
            return Some(value);
        }

        let mem = match self.maps.get_mem_by_addr(self.regs.get_esp()) {
            Some(m) => m,
            None => {
                println!(
                    "poping stack outside map  esp: 0x{:x}",
                    self.regs.get_esp() as u32
                );
                self.spawn_console();
                return None;
            }
        };*/

        let value = match self.maps.read_dword(self.regs.get_esp()) {
            Some(v) => v,
            None => {
                println!("esp point to non mapped mem");
                return None;
            }
        };

        if self.cfg.verbose >= 1
            && pop_instruction
            && self.maps.get_mem("code").inside(value.into())
        {
            println!("/!\\ poping a code address 0x{:x}", value);
        }

        if self.cfg.trace_mem {
            let name = match self.maps.get_addr_name(self.regs.get_esp()) {
                Some(n) => n,
                None => "not mapped".to_string(),
            };
            println!("\tmem_trace: pos = {} rip = {:x} op = read bits = {} address = 0x{:x} value = 0x{:x} name = '{}'", self.pos, self.regs.rip, 32, self.regs.get_esp(), value, name);
        }

        self.regs.set_esp(self.regs.get_esp() + 4);
        Some(value)
    }

    pub fn stack_pop64(&mut self, pop_instruction: bool) -> Option<u64> {
        if self.cfg.stack_trace {
            println!("--- stack pop64 ---");
            self.maps.dump_qwords(self.regs.rsp, 5);
        }

        /*
        let stack = self.maps.get_mem("stack");
        if stack.inside(self.regs.rsp) {
            let value = stack.read_qword(self.regs.rsp);
            if self.cfg.verbose >= 1
                && pop_instruction
                && self.maps.get_mem("code").inside(value.into())
            {
                println!("/!\\ poping a code address 0x{:x}", value);
            }
            self.regs.rsp += 8;
            return Some(value);
        }

        let mem = match self.maps.get_mem_by_addr(self.regs.rsp) {
            Some(m) => m,
            None => {
                println!("poping stack outside map  esp: 0x{:x}", self.regs.rsp);
                self.spawn_console();
                return None;
            }
        };

        let value = mem.read_qword(self.regs.rsp);
        */

        let value = match self.maps.read_qword(self.regs.rsp) {
            Some(v) => v,
            None => {
                println!("rsp point to non mapped mem");
                return None;
            }
        };

        if self.cfg.trace_mem {
            let name = match self.maps.get_addr_name(self.regs.rsp) {
                Some(n) => n,
                None => "not mapped".to_string(),
            };
            println!("\tmem_trace: pos = {} rip = {:x} op = read bits = {} address = 0x{:x} value = 0x{:x} name = '{}'", self.pos, self.regs.rip, 32, self.regs.rsp, value, name);
        }

        self.regs.rsp += 8;
        Some(value)
    }

    // this is not used on the emulation
    pub fn memory_operand_to_address(&mut self, operand: &str) -> u64 {
        let spl: Vec<&str> = operand.split('[').collect::<Vec<&str>>()[1]
            .split(']')
            .collect::<Vec<&str>>()[0]
            .split(' ')
            .collect();

        if operand.contains("fs:[") || operand.contains("gs:[") {
            let mem = operand.split(':').collect::<Vec<&str>>()[1];
            let value = self.memory_operand_to_address(mem);

            /*
            fs:[0x30]

            FS:[0x00] : Current SEH Frame
            FS:[0x18] : TEB (Thread Environment Block)
            FS:[0x20] : PID
            FS:[0x24] : TID
            FS:[0x30] : PEB (Process Environment Block)
            FS:[0x34] : Last Error Value
            */

            //let inm = self.get_inmediate(spl[0]);
            if self.cfg.verbose >= 1 {
                println!("FS ACCESS TO 0x{:x}", value);
            }

            if value == 0x30 {
                // PEB
                if self.cfg.verbose >= 1 {
                    println!("ACCESS TO PEB");
                }
                let peb = self.maps.get_mem("peb");
                return peb.get_base();
            }

            if value == 0x18 {
                if self.cfg.verbose >= 1 {
                    println!("ACCESS TO TEB");
                }
                let teb = self.maps.get_mem("teb");
                return teb.get_base();
            }

            if value == 0x2c {
                if self.cfg.verbose >= 1 {
                    println!("ACCESS TO CURRENT LOCALE");
                }
                return constants::EN_US_LOCALE as u64;
            }

            if value == 0xc0 {
                if self.cfg.verbose >= 1 {
                    println!("CHECKING IF ITS 32bits (ISWOW64)");
                }

                if self.cfg.is_64bits {
                    return 0;
                }

                return 1;
            }

            panic!("not implemented: {}", operand);
        }

        if spl.len() == 3 {
            //ie eax + 0xc
            let sign = spl[1];

            // weird case: [esi + eax*4]
            if spl[2].contains('*') {
                let spl2: Vec<&str> = spl[2].split('*').collect();
                if spl2.len() != 2 {
                    panic!(
                        "case ie [esi + eax*4] bad parsed the *  operand:{}",
                        operand
                    );
                }

                let reg1_val = self.regs.get_by_name(spl[0]);
                let reg2_val = self.regs.get_by_name(spl2[0]);
                let num = u64::from_str_radix(spl2[1].trim_start_matches("0x"), 16)
                    .expect("bad num conversion");

                if sign != "+" && sign != "-" {
                    panic!("weird sign2 {}", sign);
                }

                if sign == "+" {
                    return reg1_val + (reg2_val * num);
                }

                if sign == "-" {
                    return reg1_val - (reg2_val * num);
                }

                unimplemented!();
            }

            let reg = spl[0];
            let sign = spl[1];
            //println!("disp --> {}  operand:{}", spl[2], operand);
            let disp: u64;
            if self.regs.is_reg(spl[2]) {
                disp = self.regs.get_by_name(spl[2]);
            } else {
                disp = u64::from_str_radix(spl[2].trim_start_matches("0x"), 16).expect("bad disp");
            }

            if sign != "+" && sign != "-" {
                panic!("weird sign {}", sign);
            }

            if sign == "+" {
                let r: u64 = self.regs.get_by_name(reg) as u64 + disp as u64;
                return r & 0xffffffff;
            } else {
                return self.regs.get_by_name(reg) - disp;
            }
        }

        if spl.len() == 1 {
            //ie [eax]
            let reg = spl[0];

            if reg.contains("0x") {
                let addr: u64 =
                    u64::from_str_radix(reg.trim_start_matches("0x"), 16).expect("bad disp2");
                return addr;
                // weird but could be a hardcoded address [0x11223344]
            }

            let reg_val = self.regs.get_by_name(reg);
            return reg_val;
        }

        0
    }

    // this is not used on the emulation
    pub fn memory_read(&mut self, operand: &str) -> Option<u64> {
        if operand.contains("fs:[0]") {
            if self.cfg.verbose >= 1 {
                println!("{} Reading SEH fs:[0] 0x{:x}", self.pos, self.seh);
            }
            return Some(self.seh);
        }

        let addr: u64 = self.memory_operand_to_address(operand);

        if operand.contains("fs:[") || operand.contains("gs:[") {
            return Some(addr);
        }

        let bits = self.get_size(operand);
        // check integrity of eip, esp and ebp registers

        let stack = self.maps.get_mem("stack");

        // could be normal using part of code as stack
        if !stack.inside(self.regs.get_esp()) {
            //hack: redirect stack
            self.regs.set_esp(stack.get_base() + 0x1ff);
            panic!("/!\\ fixing stack.")
        }

        match bits {
            64 => match self.maps.read_qword(addr) {
                Some(v) => {
                    if self.cfg.trace_mem {
                        let name = match self.maps.get_addr_name(addr) {
                            Some(n) => n,
                            None => "not mapped".to_string(),
                        };
                        println!("\tmem_trace: pos = {} rip = {:x} op = read bits = {} address = 0x{:x} value = 0x{:x} name = '{}'", self.pos, self.regs.rip, operand, addr, v, name);
                    }
                    return Some(v);
                }
                None => return None,
            },
            32 => match self.maps.read_dword(addr) {
                Some(v) => {
                    if self.cfg.trace_mem {
                        let name = match self.maps.get_addr_name(addr) {
                            Some(n) => n,
                            None => "not mapped".to_string(),
                        };
                        println!("\tmem_trace: pos = {} rip = {:x} op = read bits = {} address = 0x{:x} value = 0x{:x} name = '{}'", self.pos, self.regs.rip, operand, addr, v, name);
                    }
                    return Some(v.into());
                }
                None => return None,
            },
            16 => match self.maps.read_word(addr) {
                Some(v) => {
                    if self.cfg.trace_mem {
                        let name = match self.maps.get_addr_name(addr) {
                            Some(n) => n,
                            None => "not mapped".to_string(),
                        };
                        println!("\tmem_trace: pos = {} rip = {:x} op = read bits = {} address = 0x{:x} value = 0x{:x} name = '{}'", self.pos, self.regs.rip, operand, addr, v, name);
                    }
                    return Some(v.into());
                }
                None => return None,
            },
            8 => match self.maps.read_byte(addr) {
                Some(v) => {
                    if self.cfg.trace_mem {
                        let name = match self.maps.get_addr_name(addr) {
                            Some(n) => n,
                            None => "not mapped".to_string(),
                        };
                        println!("\tmem_trace: pos = {} rip = {:x} op = read bits = {} address = 0x{:x} value = 0x{:x} name = '{}'", self.pos, self.regs.rip, operand, addr, v, name);
                    }
                    return Some(v.into());
                }
                None => return None,
            },
            _ => panic!("weird size: {}", operand),
        };
    }

    // this is not used on the emulation
    pub fn memory_write(&mut self, operand: &str, value: u64) -> bool {
        if operand.contains("fs:[0]") {
            println!("Setting SEH fs:[0]  0x{:x}", value);
            self.seh = value;
            return true;
        }

        let addr: u64 = self.memory_operand_to_address(operand);

        /*if !self.maps.is_mapped(addr) {
        panic!("writting in non mapped memory");
        }*/

        let name = match self.maps.get_addr_name(addr) {
            Some(n) => n,
            None => "error".to_string(),
        };

        if name == "code" {
            if self.cfg.verbose >= 1 {
                println!("/!\\ polymorfic code");
            }
            self.force_break = true;
        }

        if self.cfg.trace_mem {
            println!("\tmem_trace: pos = {} rip = {:x} op = write bits = {} address = 0x{:x} value = 0x{:x} name = '{}'", self.pos, self.regs.rip, operand, addr, value, name);
        }

        let bits = self.get_size(operand);
        let ret = match bits {
            64 => self.maps.write_qword(addr, value),
            32 => self.maps.write_dword(addr, (value & 0xffffffff) as u32),
            16 => self.maps.write_word(addr, (value & 0x0000ffff) as u16),
            8 => self.maps.write_byte(addr, (value & 0x000000ff) as u8),
            _ => unreachable!("weird size: {}", operand),
        };

        ret
    }

    // this is not used on the emulation
    pub fn get_size(&self, operand: &str) -> u8 {
        if operand.contains("byte ptr") {
            return 8;
        } else if operand.contains("dword ptr") {
            return 32;
        } else if operand.contains("qword ptr") {
            return 64;
        } else if operand.contains("word ptr") {
            return 16;
        }

        let c: Vec<char> = operand.chars().collect();

        if operand.len() == 3 {
            if c[0] == 'e' {
                return 32;
            }
        } else if operand.len() == 2 {
            if c[1] == 'x' {
                return 16;
            }

            if c[1] == 'h' || c[1] == 'l' {
                return 8;
            }

            if c[1] == 'i' {
                return 16;
            }
        }

        panic!("weird size: {}", operand);
    }

    pub fn set_rip(&mut self, addr: u64, is_branch: bool) -> bool {
        self.force_reload = true;

        if addr == constants::RETURN_THREAD.into() {
            println!("/!\\ Thread returned, continuing the main thread");
            self.regs.rip = self.main_thread_cont;
            self.spawn_console();
            self.force_break = true;
            return true;
        }

        let name = match self.maps.get_addr_name(addr) {
            Some(n) => n,
            None => {
                eprintln!("/!\\ setting rip to non mapped addr 0x{:x}", addr);
                self.exception();
                return false;
            }
        };

        let map_name = self.filename_to_mapname(&self.cfg.filename);
        if addr < constants::LIBS_BARRIER64 || name == "code" || name.starts_with(&map_name) {
            //println!("ha pasado el if {} < {} {} starts_with:{} {}", addr, constants::LIBS_BARRIER64, name, map_name, self.cfg.filename);
            self.regs.rip = addr;
            //self.force_break = true;
        } else {
            if self.linux {
                self.regs.rip = addr; // in linux libs are no implemented are emulated
            } else {
                if self.cfg.verbose >= 1 {
                    println!("/!\\ changing RIP to {} ", name);
                }

                if self.skip_apicall {
                    self.its_apicall = Some(addr);
                    return false;
                }

                self.gateway_return = self.stack_pop64(false).unwrap_or(0);
                self.regs.rip = self.gateway_return;

                let handle_winapi: bool = match self.hook.hook_on_winapi_call {
                    Some(hook_fn) => hook_fn(self, self.regs.rip, addr),
                    None => true,
                };

                if handle_winapi {
                    winapi64::gateway(addr, name, self);
                }
                self.force_break = true;
            }
        }

        return true;
    }

    pub fn set_eip(&mut self, addr: u64, is_branch: bool) -> bool {
        self.force_reload = true;

        if addr == constants::RETURN_THREAD.into() {
            println!("/!\\ Thread returned, continuing the main thread");
            self.regs.rip = self.main_thread_cont;
            self.spawn_console();
            self.force_break = true;
            return true;
        }

        let name = match self.maps.get_addr_name(addr) {
            Some(n) => n,
            None => {
                eprintln!("/!\\ setting eip to non mapped addr 0x{:x}", addr);
                self.exception();
                return false;
            }
        };

        let map_name = self.filename_to_mapname(&self.filename);
        if name == "code" || addr < constants::LIBS_BARRIER || 
            (map_name != "" && name.starts_with(&map_name)) {

            /*
            if  addr >= constants::LIBS_BARRIER {
                println!("/!\\ alert, jumping the barrier 0x{:x} name:{} map_name:{} filename:{}",
                         addr, name, map_name, &self.filename);
                if name == "code" {
                    println!("warning the name is code");
                }
                if name.starts_with(&map_name) {
                    println!("alert {} start with {}", name, map_name);
                }
                self.spawn_console();
            }*/
            /*
            println!(
                "entra map:`{}` map_name:`{}` filename:`{}` !!!",
                name, map_name, &self.filename
            );*/
            self.regs.set_eip(addr);
        } else {
            if self.cfg.verbose >= 1 {
                println!("/!\\ changing EIP to {} 0x{:x}", name, addr);
            }

            if self.skip_apicall {
                self.its_apicall = Some(addr);
                return false;
            }

            self.gateway_return = self.stack_pop32(false).unwrap_or(0).into();
            self.regs.set_eip(self.gateway_return);

            let handle_winapi: bool = match self.hook.hook_on_winapi_call {
                Some(hook_fn) => hook_fn(self, self.regs.rip, addr),
                None => true,
            };

            if handle_winapi {
                winapi32::gateway(to32!(addr), name, self);
            }

            self.force_break = true;
        }
        return true;
    }

    fn rol(&mut self, val: u64, rot2: u64, bits: u32) -> u64 {
        let mut ret: u64 = val;
        let rot;
        if bits == 64 {
            rot = rot2 & 0b111111;
        } else {
            rot = rot2 & 0b11111;
        }

        for _ in 0..rot {
            let last_bit = get_bit!(ret, bits - 1);
            //println!("last bit: {}", last_bit);
            let mut ret2: u64 = ret;

            //  For the ROL and ROR instructions, the original value of the CF flag is not a part of the result, but the CF flag receives a copy of the bit that was shifted from one end to the other.
            self.flags.f_cf = last_bit == 1;

            for j in 0..bits - 1 {
                let bit = get_bit!(ret, j);
                set_bit!(ret2, j + 1, bit);
            }

            set_bit!(ret2, 0, last_bit);
            ret = ret2;
            //println!("{:b}", ret);
        }

        ret
    }

    fn rcl(&self, val: u64, rot2: u64, bits: u32) -> u64 {
        let mut ret: u128 = val as u128;
        let rot;
        if bits == 64 {
            rot = rot2 & 0b111111;
        } else {
            rot = rot2 & 0b11111;
        }

        if self.flags.f_cf {
            set_bit!(ret, bits, 1);
        } else {
            set_bit!(ret, bits, 0);
        }

        for _ in 0..rot {
            let last_bit = get_bit!(ret, bits);
            //println!("last bit: {}", last_bit);
            let mut ret2: u128 = ret;

            for j in 0..bits {
                let bit = get_bit!(ret, j);
                set_bit!(ret2, j + 1, bit);
            }

            set_bit!(ret2, 0, last_bit);
            ret = ret2;
            //println!("{:b}", ret);
        }

        let a: u128 = 2;
        (ret & (a.pow(bits as u32) - 1)) as u64
    }

    fn ror(&mut self, val: u64, rot2: u64, bits: u32) -> u64 {
        let mut ret: u64 = val;
        let rot;
        if bits == 64 {
            rot = rot2 & 0b111111;
        } else {
            rot = rot2 & 0b11111;
        }

        for _ in 0..rot {
            let first_bit = get_bit!(ret, 0);
            let mut ret2: u64 = ret;

            //  For the ROL and ROR instructions, the original value of the CF flag is not a part of the result, but the CF flag receives a copy of the bit that was shifted from one end to the other.
            self.flags.f_cf = first_bit == 1;

            for j in (1..bits).rev() {
                let bit = get_bit!(ret, j);
                set_bit!(ret2, j - 1, bit);
            }

            set_bit!(ret2, bits - 1, first_bit);
            ret = ret2;
        }

        ret
    }

    fn rcr(&mut self, val: u64, rot2: u64, bits: u32) -> u64 {
        let mut ret: u128 = val as u128;
        let rot;
        if bits == 64 {
            rot = rot2 & 0b111111;
        } else {
            rot = rot2 & 0b11111;
        }

        if self.flags.f_cf {
            set_bit!(ret, bits, 1);
        } else {
            set_bit!(ret, bits, 0);
        }

        for _ in 0..rot {
            let first_bit = get_bit!(ret, 0);
            let mut ret2: u128 = ret;

            for j in (1..=bits).rev() {
                let bit = get_bit!(ret, j);
                set_bit!(ret2, j - 1, bit);
            }

            set_bit!(ret2, bits, first_bit);
            ret = ret2;
        }

        let cnt = rot2 % (bits + 1) as u64;
        if cnt == 1 {
            self.flags.f_cf = (val & 0x1) == 1;
        } else {
            self.flags.f_cf = ((val >> (cnt - 1)) & 0x1) == 1;
        }

        let a: u128 = 2;
        (ret & (a.pow(bits as u32) - 1)) as u64
    }

    fn mul64(&mut self, value0: u64) {
        let value1: u64 = self.regs.rax;
        let value2: u64 = value0;
        let res: u128 = value1 as u128 * value2 as u128;
        self.regs.rdx = ((res & 0xffffffffffffffff0000000000000000) >> 64) as u64;
        self.regs.rax = (res & 0xffffffffffffffff) as u64;
        self.flags.calc_pf(res as u8);
        self.flags.f_of = self.regs.rdx != 0;
        self.flags.f_cf = self.regs.rdx != 0;
    }

    fn mul32(&mut self, value0: u64) {
        let value1: u32 = to32!(self.regs.get_eax());
        let value2: u32 = value0 as u32;
        let res: u64 = value1 as u64 * value2 as u64;
        self.regs.set_edx((res & 0xffffffff00000000) >> 32);
        self.regs.set_eax(res & 0x00000000ffffffff);
        self.flags.calc_pf(res as u8);
        self.flags.f_of = self.regs.get_edx() != 0;
        self.flags.f_cf = self.regs.get_edx() != 0;
    }

    fn mul16(&mut self, value0: u64) {
        let value1: u32 = to32!(self.regs.get_ax());
        let value2: u32 = value0 as u32;
        let res: u32 = value1 * value2;
        self.regs.set_dx(((res & 0xffff0000) >> 16).into());
        self.regs.set_ax((res & 0xffff).into());
        self.flags.calc_pf(res as u8);
        self.flags.f_of = self.regs.get_dx() != 0;
        self.flags.f_cf = self.regs.get_dx() != 0;
    }

    fn mul8(&mut self, value0: u64) {
        let value1: u32 = self.regs.get_al() as u32;
        let value2: u32 = value0 as u32;
        let res: u32 = value1 * value2;
        self.regs.set_ax((res & 0xffff).into());
        self.flags.calc_pf(res as u8);
        self.flags.f_of = self.regs.get_ah() != 0;
        self.flags.f_cf = self.regs.get_ah() != 0;
    }

    fn imul64p1(&mut self, value0: u64) {
        let value1: i64 = self.regs.rax as i64;
        let value2: i64 = value0 as i64;
        let res: i128 = value1 as i128 * value2 as i128;
        let ures: u128 = res as u128;
        self.regs.rdx = ((ures & 0xffffffffffffffff0000000000000000) >> 64) as u64;
        self.regs.rax = (ures & 0xffffffffffffffff) as u64;
        self.flags.calc_pf(ures as u8);
        self.flags.f_of = self.regs.get_edx() != 0;
        self.flags.f_cf = self.regs.get_edx() != 0;
    }

    fn imul32p1(&mut self, value0: u64) {
        let value1: i32 = self.regs.get_eax() as i32;
        let value2: i32 = value0 as i32;
        let res: i64 = value1 as i64 * value2 as i64;
        let ures: u64 = res as u64;
        self.regs.set_edx((ures & 0xffffffff00000000) >> 32);
        self.regs.set_eax(ures & 0x00000000ffffffff);
        self.flags.calc_pf(ures as u8);
        self.flags.f_of = self.regs.get_edx() != 0;
        self.flags.f_cf = self.regs.get_edx() != 0;
    }

    fn imul16p1(&mut self, value0: u64) {
        let value1: i32 = self.regs.get_ax() as i32;
        let value2: i32 = value0 as i32;
        let res: i32 = value1 * value2;
        let ures: u32 = res as u32;
        self.regs.set_dx(((ures & 0xffff0000) >> 16).into());
        self.regs.set_ax((ures & 0xffff).into());
        self.flags.calc_pf(ures as u8);
        self.flags.f_of = self.regs.get_dx() != 0;
        self.flags.f_cf = self.regs.get_dx() != 0;
    }

    fn imul8p1(&mut self, value0: u64) {
        let value1: i32 = self.regs.get_al() as i32;
        let value2: i32 = value0 as i32;
        let res: i32 = value1 * value2;
        let ures: u32 = res as u32;
        self.regs.set_ax((ures & 0xffff).into());
        self.flags.calc_pf(ures as u8);
        self.flags.f_of = self.regs.get_ah() != 0;
        self.flags.f_cf = self.regs.get_ah() != 0;
    }

    fn div64(&mut self, value0: u64) {
        let mut value1: u128 = self.regs.rdx as u128;
        value1 <<= 64;
        value1 += self.regs.rax as u128;
        let value2: u128 = value0 as u128;

        if value2 == 0 {
            self.flags.f_tf = true;
            println!("/!\\ division by 0 exception");
            self.exception();
            self.force_break = true;
            return;
        }

        let resq: u128 = value1 / value2;
        let resr: u128 = value1 % value2;
        self.regs.rax = resq as u64;
        self.regs.rdx = resr as u64;
        self.flags.calc_pf(resq as u8);
        self.flags.f_of = resq > 0xffffffffffffffff;
        if self.flags.f_of {
            println!("/!\\ int overflow on division");
        }
    }

    fn div32(&mut self, value0: u64) {
        let mut value1: u64 = self.regs.get_edx();
        value1 <<= 32;
        value1 += self.regs.get_eax();
        let value2: u64 = value0;

        if value2 == 0 {
            self.flags.f_tf = true;
            println!("/!\\ division by 0 exception");
            self.exception();
            self.force_break = true;
            return;
        }

        let resq: u64 = value1 / value2;
        let resr: u64 = value1 % value2;
        self.regs.set_eax(resq);
        self.regs.set_edx(resr);
        self.flags.calc_pf(resq as u8);
        self.flags.f_of = resq > 0xffffffff;
        if self.flags.f_of {
            println!("/!\\ int overflow on division");
        }
    }

    fn div16(&mut self, value0: u64) {
        let value1: u32 = to32!((self.regs.get_dx() << 16) + self.regs.get_ax());
        let value2: u32 = value0 as u32;

        if value2 == 0 {
            self.flags.f_tf = true;
            println!("/!\\ division by 0 exception");
            self.exception();
            self.force_break = true;
            return;
        }

        let resq: u32 = value1 / value2;
        let resr: u32 = value1 % value2;
        self.regs.set_ax(resq.into());
        self.regs.set_dx(resr.into());
        self.flags.calc_pf(resq as u8);
        self.flags.f_of = resq > 0xffff;
        self.flags.f_tf = false;
        if self.flags.f_of {
            println!("/!\\ int overflow on division");
        }
    }

    fn div8(&mut self, value0: u64) {
        let value1: u32 = self.regs.get_ax() as u32;
        let value2: u32 = value0 as u32;
        if value2 == 0 {
            self.flags.f_tf = true;
            println!("/!\\ division by 0 exception");
            self.exception();
            self.force_break = true;
            return;
        }

        let resq: u32 = value1 / value2;
        let resr: u32 = value1 % value2;
        self.regs.set_al(resq.into());
        self.regs.set_ah(resr.into());
        self.flags.calc_pf(resq as u8);
        self.flags.f_of = resq > 0xff;
        self.flags.f_tf = false;
        if self.flags.f_of {
            println!("/!\\ int overflow");
        }
    }

    fn idiv64(&mut self, value0: u64) {
        let mut value1: u128 = self.regs.rdx as u128;
        value1 <<= 64;
        value1 += self.regs.rax as u128;
        let value2: u128 = value0 as u128;
        if value2 == 0 {
            self.flags.f_tf = true;
            println!("/!\\ division by 0 exception");
            self.exception();
            self.force_break = true;
            return;
        }

        let resq: u128 = value1 / value2;
        let resr: u128 = value1 % value2;
        self.regs.rax = resq as u64;
        self.regs.rdx = resr as u64;
        self.flags.calc_pf(resq as u8);
        if resq > 0xffffffffffffffff {
            println!("/!\\ int overflow exception on division");
            if self.break_on_alert {
                panic!();
            }
        } else if ((value1 as i128) > 0 && (resq as i64) < 0)
            || ((value1 as i128) < 0 && (resq as i64) > 0)
        {
            println!("/!\\ sign change exception on division");
            self.exception();
            self.force_break = true;
        }
    }

    fn idiv32(&mut self, value0: u64) {
        let mut value1: u64 = self.regs.get_edx();
        value1 <<= 32;
        value1 += self.regs.get_eax();
        let value2: u64 = value0;
        if value2 == 0 {
            self.flags.f_tf = true;
            println!("/!\\ division by 0 exception");
            self.exception();
            self.force_break = true;
            return;
        }

        let resq: u64 = value1 / value2;
        let resr: u64 = value1 % value2;
        self.regs.set_eax(resq);
        self.regs.set_edx(resr);
        self.flags.calc_pf(resq as u8);
        if resq > 0xffffffff {
            println!("/!\\ int overflow exception on division");
            if self.break_on_alert {
                panic!();
            }
        } else if ((value1 as i64) > 0 && (resq as i32) < 0)
            || ((value1 as i64) < 0 && (resq as i32) > 0)
        {
            println!("/!\\ sign change exception on division");
            self.exception();
            self.force_break = true;
        }
    }

    fn idiv16(&mut self, value0: u64) {
        let value1: u32 = to32!((self.regs.get_dx() << 16) + self.regs.get_ax());
        let value2: u32 = value0 as u32;
        if value2 == 0 {
            self.flags.f_tf = true;
            println!("/!\\ division by 0 exception");
            self.exception();
            self.force_break = true;
            return;
        }

        let resq: u32 = value1 / value2;
        let resr: u32 = value1 % value2;
        self.regs.set_ax(resq.into());
        self.regs.set_dx(resr.into());
        self.flags.calc_pf(resq as u8);
        self.flags.f_tf = false;
        if resq > 0xffff {
            println!("/!\\ int overflow exception on division");
            if self.break_on_alert {
                panic!();
            }
        } else if ((value1 as i32) > 0 && (resq as i16) < 0)
            || ((value1 as i32) < 0 && (resq as i16) > 0)
        {
            println!("/!\\ sign change exception on division");
            self.exception();
            self.force_break = true;
        }
    }

    fn idiv8(&mut self, value0: u64) {
        let value1: u32 = to32!(self.regs.get_ax());
        let value2: u32 = value0 as u32;
        if value2 == 0 {
            self.flags.f_tf = true;
            println!("/!\\ division by 0 exception");
            self.exception();
            self.force_break = true;
            return;
        }

        let resq: u32 = value1 / value2;
        let resr: u32 = value1 % value2;
        self.regs.set_al(resq.into());
        self.regs.set_ah(resr.into());
        self.flags.calc_pf(resq as u8);
        self.flags.f_tf = false;
        if resq > 0xff {
            println!("/!\\ int overflow exception on division");
            if self.break_on_alert {
                panic!();
            }
        } else if ((value1 as i16) > 0 && (resq as i8) < 0)
            || ((value1 as i16) < 0 && (resq as i8) > 0)
        {
            println!("/!\\ sign change exception on division");
            self.exception();
            self.force_break = true;
        }
    }

    pub fn shrd(&mut self, value0: u64, value1: u64, pcounter: u64, size: u32) -> (u64, bool) {
        let mut storage0: u64 = value0;
        let mut counter: u64 = pcounter;

        /*if size == 64 {
            counter = counter % 64;
        } else {
            counter = counter % 32;
        }*/

        match size {
            64 => counter = counter % 64,
            32 => counter = counter % 32,
            _ => {}
        }

        if counter == 0 {
            return (storage0, false);
        }

        if counter >= size as u64 {
            if self.cfg.verbose >= 1 {
                println!("/!\\ SHRD undefined behaviour value0 = 0x{:x} value1 = 0x{:x} pcounter = 0x{:x} counter = 0x{:x} size = 0x{:x}", value0, value1, pcounter, counter, size);
            }
            let result = 0; //inline::shrd(value0, value1, pcounter, size);
            self.flags.calc_flags(result, size);
            return (result, true);
        }

        self.flags.f_cf = get_bit!(value0, counter - 1) == 1;

        let mut to = size as u64 - 1 - counter;
        if to > 64 {
            // println!("to: {}", to);
            to = 64;
        }

        for i in 0..=to {
            let bit = get_bit!(storage0, i as u32 + counter as u32);
            set_bit!(storage0, i as u32, bit);
        }

        let from = size as u64 - counter;

        //println!("from: {}", from);

        for i in from..size as u64 {
            let bit = get_bit!(value1, i as u32 + counter as u32 - size as u32);
            set_bit!(storage0, i as u32, bit);
        }

        /*
        for i in 0..=(size as u64 -1 -counter) {
           let bit = get_bit!(storage0, i+counter);
           set_bit!(storage0, i, bit);
        }
        for i in (size as u64 -counter)..(size as u64) {
            let bit = get_bit!(storage0, i+counter-size as u64);
            set_bit!(storage0, i, bit);
        }*/

        self.flags.calc_flags(storage0, size.into());
        (storage0, false)
    }

    pub fn shld(&mut self, value0: u64, value1: u64, pcounter: u64, size: u32) -> (u64, bool) {
        let mut storage0: u64 = value0;
        let mut counter: u64 = pcounter;

        if size == 64 {
            counter = counter % 64;
        } else {
            counter = counter % 32;
        }

        if counter == 0 {
            return (value0, false);
        }

        /*
        if counter >= size as u64 {
            counter = size as u64 -1;
        }*/

        if counter > size as u64 {
            if self.cfg.verbose >= 1 {
                println!("/!\\ undefined behaviour on shld");
            }

            let result = 0;
            //let result = inline::shld(value0, value1, pcounter, size);
            self.flags.calc_flags(result, size);

            return (result, true);
            //counter = pcounter - size as u64;
        }

        self.flags.f_cf = get_bit!(value0, size as u64 - counter) == 1;
        /*
        if counter < size as u64 && size - (counter as u8) < 64 {
            self.flags.f_cf = get_bit!(value0, size - counter as u8) == 1;
        }*/

        for i in (counter..=((size as u64) - 1)).rev() {
            let bit = get_bit!(storage0, i - counter);
            set_bit!(storage0, i, bit);
        }

        for i in (0..counter).rev() {
            let bit = get_bit!(value1, i + (size as u64) - counter);
            set_bit!(storage0, i, bit);
        }

        self.flags.calc_flags(storage0, size);

        (storage0, false)
    }

    pub fn spawn_console(&mut self) {
        if !self.cfg.console_enabled {
            return;
        }

        let con = Console::new();
        if self.pos > 0 {
            self.pos -= 1;
        }
        loop {
            let cmd = con.cmd();
            match cmd.as_str() {
                "q" => std::process::exit(1),
                "h" => con.help(),
                "r" => {
                    if self.cfg.is_64bits {
                        self.featured_regs64();
                    } else {
                        self.featured_regs32();
                    }
                }
                "r rax" => self.regs.show_rax(&self.maps, 0),
                "r rbx" => self.regs.show_rbx(&self.maps, 0),
                "r rcx" => self.regs.show_rcx(&self.maps, 0),
                "r rdx" => self.regs.show_rdx(&self.maps, 0),
                "r rsi" => self.regs.show_rsi(&self.maps, 0),
                "r rdi" => self.regs.show_rdi(&self.maps, 0),
                "r rbp" => println!("\trbp: 0x{:x}", self.regs.rbp),
                "r rsp" => println!("\trsp: 0x{:x}", self.regs.rsp),
                "r rip" => println!("\trip: 0x{:x}", self.regs.rip),
                "r eax" => self.regs.show_eax(&self.maps, 0),
                "r ebx" => self.regs.show_ebx(&self.maps, 0),
                "r ecx" => self.regs.show_ecx(&self.maps, 0),
                "r edx" => self.regs.show_edx(&self.maps, 0),
                "r esi" => self.regs.show_esi(&self.maps, 0),
                "r edi" => self.regs.show_edi(&self.maps, 0),
                "r esp" => println!("\tesp: 0x{:x}", self.regs.get_esp() as u32),
                "r ebp" => println!("\tebp: 0x{:x}", self.regs.get_ebp() as u32),
                "r eip" => println!("\teip: 0x{:x}", self.regs.get_eip() as u32),
                "r r8" => self.regs.show_r8(&self.maps, 0),
                "r r9" => self.regs.show_r9(&self.maps, 0),
                "r r10" => self.regs.show_r10(&self.maps, 0),
                "r r11" => self.regs.show_r11(&self.maps, 0),
                "r r12" => self.regs.show_r12(&self.maps, 0),
                "r r13" => self.regs.show_r13(&self.maps, 0),
                "r r14" => self.regs.show_r14(&self.maps, 0),
                "r r15" => self.regs.show_r15(&self.maps, 0),
                "r r8d" => self.regs.show_r8d(&self.maps, 0),
                "r r9d" => self.regs.show_r9d(&self.maps, 0),
                "r r10d" => self.regs.show_r10d(&self.maps, 0),
                "r r11d" => self.regs.show_r11d(&self.maps, 0),
                "r r12d" => self.regs.show_r12d(&self.maps, 0),
                "r r13d" => self.regs.show_r13d(&self.maps, 0),
                "r r14d" => self.regs.show_r14d(&self.maps, 0),
                "r r15d" => self.regs.show_r15d(&self.maps, 0),
                "r r8w" => self.regs.show_r8w(&self.maps, 0),
                "r r9w" => self.regs.show_r9w(&self.maps, 0),
                "r r10w" => self.regs.show_r10w(&self.maps, 0),
                "r r11w" => self.regs.show_r11w(&self.maps, 0),
                "r r12w" => self.regs.show_r12w(&self.maps, 0),
                "r r13w" => self.regs.show_r13w(&self.maps, 0),
                "r r14w" => self.regs.show_r14w(&self.maps, 0),
                "r r15w" => self.regs.show_r15w(&self.maps, 0),
                "r r8l" => self.regs.show_r8l(&self.maps, 0),
                "r r9l" => self.regs.show_r9l(&self.maps, 0),
                "r r10l" => self.regs.show_r10l(&self.maps, 0),
                "r r11l" => self.regs.show_r11l(&self.maps, 0),
                "r r12l" => self.regs.show_r12l(&self.maps, 0),
                "r r13l" => self.regs.show_r13l(&self.maps, 0),
                "r r14l" => self.regs.show_r14l(&self.maps, 0),
                "r r15l" => self.regs.show_r15l(&self.maps, 0),
                "r xmm0" => println!("\txmm0: 0x{:x}", self.regs.xmm0),
                "r xmm1" => println!("\txmm1: 0x{:x}", self.regs.xmm1),
                "r xmm2" => println!("\txmm2: 0x{:x}", self.regs.xmm2),
                "r xmm3" => println!("\txmm3: 0x{:x}", self.regs.xmm3),
                "r xmm4" => println!("\txmm4: 0x{:x}", self.regs.xmm4),
                "r xmm5" => println!("\txmm5: 0x{:x}", self.regs.xmm5),
                "r xmm6" => println!("\txmm6: 0x{:x}", self.regs.xmm6),
                "r xmm7" => println!("\txmm7: 0x{:x}", self.regs.xmm7),
                "r xmm8" => println!("\txmm8: 0x{:x}", self.regs.xmm8),
                "r xmm9" => println!("\txmm9: 0x{:x}", self.regs.xmm9),
                "r xmm10" => println!("\txmm10: 0x{:x}", self.regs.xmm10),
                "r xmm11" => println!("\txmm11: 0x{:x}", self.regs.xmm11),
                "r xmm12" => println!("\txmm12: 0x{:x}", self.regs.xmm12),
                "r xmm13" => println!("\txmm13: 0x{:x}", self.regs.xmm13),
                "r xmm14" => println!("\txmm14: 0x{:x}", self.regs.xmm14),
                "r xmm15" => println!("\txmm15: 0x{:x}", self.regs.xmm15),
                "r ymm0" => println!("\tymm0: 0x{:x}", self.regs.ymm0),
                "r ymm1" => println!("\tymm1: 0x{:x}", self.regs.ymm1),
                "r ymm2" => println!("\tymm2: 0x{:x}", self.regs.ymm2),
                "r ymm3" => println!("\tymm3: 0x{:x}", self.regs.ymm3),
                "r ymm4" => println!("\tymm4: 0x{:x}", self.regs.ymm4),
                "r ymm5" => println!("\tymm5: 0x{:x}", self.regs.ymm5),
                "r ymm6" => println!("\tymm6: 0x{:x}", self.regs.ymm6),
                "r ymm7" => println!("\tymm7: 0x{:x}", self.regs.ymm7),
                "r ymm8" => println!("\tymm8: 0x{:x}", self.regs.ymm8),
                "r ymm9" => println!("\tymm9: 0x{:x}", self.regs.ymm9),
                "r ymm10" => println!("\tymm10: 0x{:x}", self.regs.ymm10),
                "r ymm11" => println!("\tymm11: 0x{:x}", self.regs.ymm11),
                "r ymm12" => println!("\tymm12: 0x{:x}", self.regs.ymm12),
                "r ymm13" => println!("\tymm13: 0x{:x}", self.regs.ymm13),
                "r ymm14" => println!("\tymm14: 0x{:x}", self.regs.ymm14),
                "r ymm15" => println!("\tymm15: 0x{:x}", self.regs.ymm15),

                "rc" => {
                    con.print("register name");
                    let reg = con.cmd();
                    con.print("value");
                    let value = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value");
                            continue;
                        }
                    };
                    self.regs.set_by_name(reg.as_str(), value);
                }
                "mr" | "rm" => {
                    con.print("memory argument");
                    let operand = con.cmd();
                    let addr: u64 = self.memory_operand_to_address(operand.as_str());
                    let value = match self.memory_read(operand.as_str()) {
                        Some(v) => v,
                        None => {
                            println!("bad address.");
                            continue;
                        }
                    };
                    println!("0x{:x}: 0x{:x}", to32!(addr), value);
                }
                "mw" | "wm" => {
                    con.print("memory argument");
                    let operand = con.cmd();
                    con.print("value");
                    let value = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    if self.memory_write(operand.as_str(), value) {
                        println!("done.");
                    } else {
                        println!("cannot write there.");
                    }
                }
                "mwb" => {
                    con.print("addr");
                    let addr = match con.cmd_hex64() {
                        Ok(a) => a,
                        Err(_) => {
                            println!("bad hex value");
                            continue;
                        }
                    };
                    con.print("spaced bytes");
                    let bytes = con.cmd();
                    self.maps.write_spaced_bytes(addr, &bytes);
                    println!("done.");
                }
                "b" => {
                    self.bp.show();
                }
                "ba" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    self.bp.set_bp(addr);
                }
                "bmr" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    self.bp.set_mem_read(addr);
                }
                "bmw" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    self.bp.set_mem_write(addr);
                }
                "bi" => {
                    con.print("instruction number");
                    let num = match con.cmd_num() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad instruction number.");
                            continue;
                        }
                    };
                    self.bp.set_instruction(num);
                    self.exp = num;
                }
                "bc" => {
                    self.bp.clear_bp();
                    self.exp = self.pos + 1;
                }
                "bcmp" => {
                    self.break_on_next_cmp = true;
                }
                "cls" => println!("{}", self.colors.clear_screen),
                "s" => {
                    if self.cfg.is_64bits {
                        self.maps.dump_qwords(self.regs.rsp, 10);
                    } else {
                        self.maps.dump_dwords(self.regs.get_esp(), 10);
                    }
                }
                "v" => {
                    if self.cfg.is_64bits {
                        self.maps.dump_qwords(self.regs.rbp - 0x100, 100);
                    } else {
                        self.maps.dump_dwords(self.regs.get_ebp() - 0x100, 100);
                    }
                    self.maps
                        .get_mem("stack")
                        .print_dwords_from_to(self.regs.get_ebp(), self.regs.get_ebp() + 0x100);
                }
                "sv" => {
                    con.print("verbose level");
                    self.cfg.verbose = match con.cmd_num() {
                        Ok(v) => to32!(v),
                        Err(_) => {
                            println!("incorrect verbose level, set 0, 1 or 2");
                            continue;
                        }
                    };
                }
                "tr" => {
                    con.print("register");
                    let reg = con.cmd();
                    self.cfg.trace_reg = true;
                    self.cfg.reg_names.push(reg);
                }
                "trc" => {
                    self.cfg.trace_reg = false;
                    self.cfg.reg_names.clear();
                }
                "c" => {
                    self.is_running.store(1, atomic::Ordering::Relaxed);
                    return;
                }
                "cr" => {
                    self.break_on_next_return = true;
                    self.is_running.store(1, atomic::Ordering::Relaxed);
                    return;
                }
                "f" => self.flags.print(),
                "fc" => self.flags.clear(),
                "fz" => self.flags.f_zf = !self.flags.f_zf,
                "fs" => self.flags.f_sf = !self.flags.f_sf,
                "mc" => {
                    con.print("name ");
                    let name = con.cmd();
                    con.print("size ");
                    let sz = match con.cmd_num() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad size.");
                            continue;
                        }
                    };

                    let addr = match self.maps.alloc(sz) {
                        Some(a) => a,
                        None => {
                            println!("memory full");
                            continue;
                        }
                    };
                    let map = self.maps.create_map(&name);
                    map.set_base(addr);
                    map.set_size(sz);
                    println!("allocated {} at 0x{:x} sz: {}", name, addr, sz);
                }
                "mca" => {
                    con.print("name ");
                    let name = con.cmd();
                    con.print("address ");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad size.");
                            continue;
                        }
                    };

                    con.print("size ");
                    let sz = match con.cmd_num() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad size.");
                            continue;
                        }
                    };

                    let map = self.maps.create_map(&name);
                    map.set_base(addr);
                    map.set_size(sz);
                    println!("allocated {} at 0x{:x} sz: {}", name, addr, sz);
                }
                "ml" => {
                    con.print("map name");
                    let name = con.cmd();
                    con.print("filename");
                    let filename = con.cmd();
                    self.maps.get_mem(name.as_str()).load(filename.as_str());
                }
                "mn" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    self.maps.show_addr_names(addr);
                    let name = match self.maps.get_addr_name(addr) {
                        Some(n) => n,
                        None => {
                            if !self.cfg.skip_unimplemented {
                                println!("address not found on any map");
                                continue;
                            }

                            "code".to_string()
                        }
                    };

                    let mem = self.maps.get_mem(name.as_str());
                    if self.cfg.is_64bits {
                        println!(
                            "map: {} 0x{:x}-0x{:x} ({})",
                            name,
                            mem.get_base(),
                            mem.get_bottom(),
                            mem.size()
                        );
                    } else {
                        println!(
                            "map: {} 0x{:x}-0x{:x} ({})",
                            name,
                            to32!(mem.get_base()),
                            to32!(mem.get_bottom()),
                            mem.size()
                        );
                    }
                }
                "ma" => {
                    self.maps.show_allocs();
                }
                "md" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    self.maps.dump(addr);
                }
                "mrd" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    self.maps.dump_dwords(addr, 10);
                }
                "mrq" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    self.maps.dump_qwords(addr, 10);
                }
                "mds" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    if self.cfg.is_64bits {
                        println!("0x{:x}: '{}'", addr, self.maps.read_string(addr));
                    } else {
                        println!("0x{:x}: '{}'", to32!(addr), self.maps.read_string(addr));
                    }
                }
                "mdw" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    if self.cfg.is_64bits {
                        println!("0x{:x}: '{}'", addr, self.maps.read_wide_string(addr));
                    } else {
                        println!(
                            "0x{:x}: '{}'",
                            to32!(addr),
                            self.maps.read_wide_string(addr)
                        );
                    }
                }
                "mdd" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    con.print("size");
                    let sz = match con.cmd_num() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad numeric decimal value.");
                            continue;
                        }
                    };
                    if sz > 0 {
                        con.print("file");
                        let filename = con.cmd();
                        self.maps.save(addr, sz, filename);
                    }
                }
                "mdda" => {
                    con.print("path:");
                    let path = con.cmd2();
                    self.maps.save_all_allocs(path);
                }
                "mt" => {
                    if self.maps.mem_test() {
                        println!("mem test passed ok.");
                    } else {
                        println!("memory errors.");
                    }
                }
                "eip" => {
                    con.print("=");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value");
                            continue;
                        }
                    };
                    //self.force_break = true;
                    //self.regs.set_eip(addr);
                    self.set_eip(addr, false);
                }
                "rip" => {
                    con.print("=");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value");
                            continue;
                        }
                    };
                    //self.force_break = true;
                    //self.regs.rip = addr;
                }
                "push" => {
                    con.print("value");
                    if self.cfg.is_64bits {
                        let value = match con.cmd_hex64() {
                            Ok(v) => v,
                            Err(_) => {
                                println!("bad hex value");
                                continue;
                            }
                        };
                        self.stack_push64(value);
                    } else {
                        let value = match con.cmd_hex32() {
                            Ok(v) => v,
                            Err(_) => {
                                println!("bad hex value");
                                continue;
                            }
                        };
                        self.stack_push32(value);
                    }
                    println!("pushed.");
                }
                "pop" => {
                    if self.cfg.is_64bits {
                        let value = self.stack_pop64(false).unwrap_or(0);
                        println!("poped value 0x{:x}", value);
                    } else {
                        let value = self.stack_pop32(false).unwrap_or(0);
                        println!("poped value 0x{:x}", value);
                    }
                }
                "fpu" => {
                    self.fpu.print();
                }
                "md5" => {
                    con.print("map name");
                    let mem_name = con.cmd();
                    let mem = self.maps.get_mem(&mem_name);
                    let md5 = mem.md5();
                    println!("md5sum: {:x}", md5);
                }
                "ss" => {
                    con.print("map name");
                    let mem_name = con.cmd();
                    con.print("string");
                    let kw = con.cmd2();
                    let result = match self.maps.search_string(&kw, &mem_name) {
                        Some(v) => v,
                        None => {
                            println!("not found.");
                            continue;
                        }
                    };
                    for addr in result.iter() {
                        if self.cfg.is_64bits {
                            println!("found 0x{:x} '{}'", *addr, self.maps.read_string(*addr));
                        } else {
                            println!(
                                "found 0x{:x} '{}'",
                                *addr as u32,
                                self.maps.read_string(*addr)
                            );
                        }
                    }
                }
                "sb" => {
                    con.print("map name");
                    let mem_name = con.cmd();
                    con.print("spaced bytes");
                    let sbs = con.cmd();
                    let results = self.maps.search_spaced_bytes(&sbs, &mem_name);
                    if results.len() == 0 {
                        println!("not found.");
                    } else {
                        if self.cfg.is_64bits {
                            for addr in results.iter() {
                                println!("found at 0x{:x}", addr);
                            }
                        } else {
                            for addr in results.iter() {
                                println!("found at 0x{:x}", to32!(addr));
                            }
                        }
                    }
                }
                "sba" => {
                    con.print("spaced bytes");
                    let sbs = con.cmd();
                    let results = self.maps.search_spaced_bytes_in_all(&sbs);
                    if results.len() == 0 {
                        println!("not found.");
                    } else {
                        if self.cfg.is_64bits {
                            for addr in results.iter() {
                                println!("found at 0x{:x}", addr);
                            }
                        } else {
                            for addr in results.iter() {
                                println!("found at 0x{:x}", to32!(addr));
                            }
                        }
                    }
                }
                "ssa" => {
                    con.print("string");
                    let kw = con.cmd2();
                    self.maps.search_string_in_all(kw);
                }
                "seh" => {
                    println!("0x{:x}", self.seh);
                }
                "veh" => {
                    println!("0x{:x}", self.veh);
                }
                "ll" => {
                    con.print("ptr");
                    let ptr1 = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value");
                            continue;
                        }
                    };
                    let mut ptr = ptr1;
                    loop {
                        println!("- 0x{:x}", ptr);
                        ptr = match self.maps.read_dword(ptr) {
                            Some(v) => v.into(),
                            None => break,
                        };
                        if ptr == 0 || ptr == ptr1 {
                            break;
                        }
                    }
                }
                "n" | "" => {
                    //self.exp = self.pos + 1;
                    let prev_verbose = self.cfg.verbose;
                    self.cfg.verbose = 3;
                    self.step();
                    self.cfg.verbose = prev_verbose;
                    //return;
                }
                "m" => self.maps.print_maps(),
                "ms" => {
                    con.print("keyword");
                    let kw = con.cmd2();
                    self.maps.print_maps_keyword(&kw);
                }
                "d" => {
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value");
                            continue;
                        }
                    };
                    println!("{}", self.disassemble(addr, 10));
                }
                "ldr" => {
                    if self.cfg.is_64bits {
                        peb64::show_linked_modules(self);
                    } else {
                        peb32::show_linked_modules(self);
                    }
                }
                "iat" => {
                    con.print("api keyword");
                    let kw = con.cmd2();
                    let addr: u64;
                    let lib: String;
                    let name: String;

                    if self.cfg.is_64bits {
                        (addr, lib, name) = winapi64::kernel32::search_api_name(self, &kw);
                    } else {
                        (addr, lib, name) = winapi32::kernel32::search_api_name(self, &kw);
                    }

                    if addr == 0 {
                        println!("api not found");
                    } else {
                        println!("found: 0x{:x} {}!{}", addr, lib, name);
                    }
                }
                "iatx" => {
                    // addr to name
                    con.print("api addr");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value.");
                            continue;
                        }
                    };
                    let name: String;

                    if self.cfg.is_64bits {
                        name = winapi64::kernel32::resolve_api_addr_to_name(self, addr);
                    } else {
                        name = winapi32::kernel32::resolve_api_addr_to_name(self, addr);
                    }

                    if name == "" {
                        println!("api addr not found");
                    } else {
                        println!("found: 0x{:x} {}", addr, name);
                    }
                }
                "iatd" => {
                    con.print("module");
                    let lib = con.cmd2().to_lowercase();
                    if self.cfg.is_64bits {
                        winapi64::kernel32::dump_module_iat(self, &lib);
                    } else {
                        winapi32::kernel32::dump_module_iat(self, &lib);
                    }
                }
                "dt" => {
                    con.print("structure");
                    let struc = con.cmd();
                    con.print("address");
                    let addr = match con.cmd_hex64() {
                        Ok(v) => v,
                        Err(_) => {
                            println!("bad hex value");
                            continue;
                        }
                    };

                    match struc.as_str() {
                        "peb" => {
                            let s = structures::PEB::load(addr, &self.maps);
                            s.print();
                        }
                        "teb" => {
                            let s = structures::TEB::load(addr, &self.maps);
                            s.print();
                        }
                        "peb_ldr_data" => {
                            let s = structures::PebLdrData::load(addr, &self.maps);
                            s.print();
                        }
                        "ldr_data_table_entry" => {
                            let s = structures::LdrDataTableEntry::load(addr, &self.maps);
                            s.print();
                        }
                        "list_entry" => {
                            let s = structures::ListEntry::load(addr, &self.maps);
                            s.print();
                        }
                        "cppeh_record" => {
                            let s = structures::CppEhRecord::load(addr, &self.maps);
                            s.print();
                        }
                        "exception_pointers" => {
                            let s = structures::ExceptionPointers::load(addr, &self.maps);
                            s.print();
                        }
                        "eh3_exception_registgration" => {
                            let s = structures::Eh3ExceptionRegistration::load(addr, &self.maps);
                            s.print();
                        }
                        "memory_basic_information" => {
                            let s = structures::MemoryBasicInformation::load(addr, &self.maps);
                            s.print();
                        }
                        "peb64" => {
                            let s = structures::PEB64::load(addr, &self.maps);
                            s.print();
                        }
                        "teb64" => {
                            let s = structures::TEB64::load(addr, &self.maps);
                            s.print();
                        }
                        "ldrdatatableentry64" => {
                            let s = structures::LdrDataTableEntry64::load(addr, &self.maps);
                            s.print();
                        }
                        "image_export_directory" => {
                            let s = structures::ImageExportDirectory::load(addr, &self.maps);
                            s.print();
                        }

                        _ => println!("unrecognized structure."),
                    }
                } // end dt command

                _ => println!("command not found, type h"),
            } // match commands
        } // end loop
    } // end commands function

    fn featured_regs32(&self) {
        self.regs.show_eax(&self.maps, 0);
        self.regs.show_ebx(&self.maps, 0);
        self.regs.show_ecx(&self.maps, 0);
        self.regs.show_edx(&self.maps, 0);
        self.regs.show_esi(&self.maps, 0);
        self.regs.show_edi(&self.maps, 0);
        println!("\tesp: 0x{:x}", self.regs.get_esp() as u32);
        println!("\tebp: 0x{:x}", self.regs.get_ebp() as u32);
        println!("\teip: 0x{:x}", self.regs.get_eip() as u32);
    }

    fn featured_regs64(&self) {
        self.regs.show_rax(&self.maps, 0);
        self.regs.show_rbx(&self.maps, 0);
        self.regs.show_rcx(&self.maps, 0);
        self.regs.show_rdx(&self.maps, 0);
        self.regs.show_rsi(&self.maps, 0);
        self.regs.show_rdi(&self.maps, 0);
        println!("\trsp: 0x{:x}", self.regs.rsp);
        println!("\trbp: 0x{:x}", self.regs.rbp);
        println!("\trip: 0x{:x}", self.regs.rip);
        self.regs.show_r8(&self.maps, 0);
        self.regs.show_r9(&self.maps, 0);
        self.regs.show_r10(&self.maps, 0);
        self.regs.show_r11(&self.maps, 0);
        self.regs.show_r12(&self.maps, 0);
        self.regs.show_r13(&self.maps, 0);
        self.regs.show_r14(&self.maps, 0);
        self.regs.show_r15(&self.maps, 0);
    }

    fn exception(&mut self) {
        let addr: u64;
        let next: u64;

        let handle_exception: bool = match self.hook.hook_on_exception {
            Some(hook_fn) => hook_fn(self, self.regs.rip),
            None => true,
        };

        /*if !handle_exception {
            return;
        }*/

        if self.veh > 0 {
            addr = self.veh;

            exception::enter(self);
            if self.cfg.is_64bits {
                self.set_rip(addr, false);
            } else {
                self.set_eip(addr, false);
            }
        } else {
            if self.seh == 0 {
                println!("exception without any SEH handler nor vector configured.");
                if self.cfg.console_enabled {
                    self.spawn_console();
                }
                return;
            }

            // SEH

            next = match self.maps.read_dword(self.seh) {
                Some(value) => value.into(),
                None => {
                    println!("exception wihout correct SEH");
                    return;
                }
            };

            addr = match self.maps.read_dword(self.seh + 4) {
                Some(value) => value.into(),
                None => {
                    println!("exception without correct SEH.");
                    return;
                }
            };

            let con = Console::new();
            con.print("jump the exception pointer (y/n)?");
            let cmd = con.cmd();
            if cmd == "y" {
                self.seh = next;
                exception::enter(self);
                if self.cfg.is_64bits {
                    self.set_rip(addr, false);
                } else {
                    self.set_eip(addr, false);
                }
            }
        }
    }

    pub fn disassemble(&mut self, addr: u64, amount: u32) -> String {
        let mut out = String::new();
        let map_name = self.maps.get_addr_name(addr).expect("address not mapped");
        let code = self.maps.get_mem(map_name.as_str());
        let block = code.read_from(addr);
        let bits: u32;
        if self.cfg.is_64bits {
            bits = 64
        } else {
            bits = 32
        }
        let mut decoder = Decoder::with_ip(bits, block, addr, DecoderOptions::NONE);
        let mut formatter = IntelFormatter::new();
        formatter.options_mut().set_digit_separator("");
        formatter.options_mut().set_first_operand_char_index(6);
        let mut output = String::new();
        let mut instruction = Instruction::default();
        let mut count: u32 = 1;
        while decoder.can_decode() {
            decoder.decode_out(&mut instruction);
            output.clear();
            formatter.format(&instruction, &mut output);
            if self.cfg.is_64bits {
                out.push_str(&format!("0x{:x}: {}\n", instruction.ip(), output));
                //println!("0x{:x}: {}", instruction.ip(), output);
            } else {
                out.push_str(&format!("0x{:x}: {}\n", instruction.ip32(), output));
                //println!("0x{:x}: {}", instruction.ip32(), output);
            }
            count += 1;
            if count == amount {
                break;
            }
        }
        return out;
    }

    pub fn get_operand_value(
        &mut self,
        ins: &Instruction,
        noperand: u32,
        do_derref: bool,
    ) -> Option<u64> {
        assert!(ins.op_count() > noperand);

        let value: u64 = match ins.op_kind(noperand) {
            OpKind::NearBranch64 => ins.near_branch64(),
            OpKind::NearBranch32 => ins.near_branch32().into(),
            OpKind::NearBranch16 => ins.near_branch16().into(),
            OpKind::FarBranch32 => ins.far_branch32().into(),
            OpKind::FarBranch16 => ins.far_branch16().into(),

            OpKind::Immediate64 => ins.immediate64() as u64,
            OpKind::Immediate8 => ins.immediate8() as u8 as u64,
            OpKind::Immediate16 => ins.immediate16() as u16 as u64,
            OpKind::Immediate32 => ins.immediate32() as u32 as u64,
            OpKind::Immediate8to64 => ins.immediate8to64() as u64,
            OpKind::Immediate32to64 => ins.immediate32to64() as u64,
            OpKind::Immediate8to32 => ins.immediate8to32() as u32 as u64,
            OpKind::Immediate8to16 => ins.immediate8to16() as u16 as u64,

            /*OpKind::Immediate64 => ins.immediate64(),
            OpKind::Immediate8 => ins.immediate8().into(),
            OpKind::Immediate16 => ins.immediate16().into(),
            OpKind::Immediate32 => ins.immediate32() as u32 as u64,
            OpKind::Immediate8to64 => ins.immediate8to64() as u64,
            OpKind::Immediate32to64 => ins.immediate32to64() as u64,
            OpKind::Immediate8to32 => ins.immediate8to32() as u32 as u64,
            OpKind::Immediate8to16 => ins.immediate8to16() as u16 as u64,
            */
            OpKind::Register => self.regs.get_reg(ins.op_register(noperand)),
            OpKind::Memory => {
                let mut derref = do_derref;
                let mut fs = false;
                let mut gs = false;

                let mut mem_addr = ins
                    .virtual_address(noperand, 0, |reg, idx, _sz| {
                        if reg == Register::FS {
                            derref = false;
                            fs = true;

                            Some(0)
                        } else if reg == Register::GS {
                            derref = false;
                            gs = true;

                            Some(0)
                        } else {
                            Some(self.regs.get_reg(reg))
                        }
                    })
                    .expect("error reading memory");

                if fs {
                    if self.linux {
                        if let Some(val) = self.fs.get(&mem_addr) {
                            if self.cfg.verbose > 0 {
                                println!("reading FS[0x{:x}] -> 0x{:x}", mem_addr, *val);
                            }
                            if *val == 0 {
                                return Some(0); //0x7ffff7ff000);
                            }
                            return Some(*val);
                        } else {
                            if self.cfg.verbose > 0 {
                                println!("reading FS[0x{:x}] -> 0", mem_addr);
                            }
                            return Some(0); //0x7ffff7fff000);
                        }
                    }

                    let value: u64 = match mem_addr {
                        0xc0 => {
                            if self.cfg.verbose >= 1 {
                                println!(
                                    "{} Reading ISWOW64 is 32bits on a 64bits system?",
                                    self.pos
                                );
                            }
                            if self.cfg.is_64bits {
                                0
                            } else {
                                1
                            }
                        }
                        0x30 => {
                            let peb = self.maps.get_mem("peb");
                            if self.cfg.verbose >= 1 {
                                println!("{} Reading PEB 0x{:x}", self.pos, peb.get_base());
                            }
                            peb.get_base()
                        }
                        0x20 => {
                            if self.cfg.verbose >= 1 {
                                println!("{} Reading PID 0x{:x}", self.pos, 10);
                            }
                            10
                        }
                        0x24 => {
                            if self.cfg.verbose >= 1 {
                                println!("{} Reading TID 0x{:x}", self.pos, 101);
                            }
                            101
                        }
                        0x34 => {
                            if self.cfg.verbose >= 1 {
                                println!("{} Reading last error value 0", self.pos);
                            }
                            0
                        }
                        0x18 => {
                            let teb = self.maps.get_mem("teb");
                            if self.cfg.verbose >= 1 {
                                println!("{} Reading TEB 0x{:x}", self.pos, teb.get_base());
                            }
                            teb.get_base()
                        }
                        0x00 => {
                            if self.cfg.verbose >= 1 {
                                println!("Reading SEH 0x{:x}", self.seh);
                            }
                            self.seh
                        }
                        0x28 => {
                            // TODO  linux TCB
                            0
                        }
                        0x2c => {
                            if self.cfg.verbose >= 1 {
                                println!("Reading local ");
                            }
                            let locale = self.alloc("locale", 100);
                            self.maps.write_dword(locale, constants::EN_US_LOCALE);
                            //TODO: return a table of locales
                            /*
                            13071 0x41026e: mov   eax,[edx+eax*4]
                            =>r edx
                                edx: 0xc8 200 (locale)
                            =>r eax
                                eax: 0x409 1033
                            */

                            locale
                        }
                        _ => {
                            println!("unimplemented fs:[{}]", mem_addr);
                            return None;
                        }
                    };
                    mem_addr = value;
                }
                if gs {
                    let value: u64 = match mem_addr {
                        0x60 => {
                            let peb = self.maps.get_mem("peb");
                            if self.cfg.verbose >= 1 {
                                println!("{} Reading PEB 0x{:x}", self.pos, peb.get_base());
                            }
                            peb.get_base()
                        }
                        0x30 => {
                            let teb = self.maps.get_mem("teb");
                            if self.cfg.verbose >= 1 {
                                println!("{} Reading TEB 0x{:x}", self.pos, teb.get_base());
                            }
                            teb.get_base()
                        }
                        0x40 => {
                            if self.cfg.verbose >= 1 {
                                println!("{} Reading PID 0x{:x}", self.pos, 10);
                            }
                            10
                        }
                        0x48 => {
                            if self.cfg.verbose >= 1 {
                                println!("{} Reading TID 0x{:x}", self.pos, 101);
                            }
                            101
                        }
                        0x10 => {
                            let stack = self.maps.get_mem("stack");
                            if self.cfg.verbose >= 1 {
                                println!("{} Reading StackLimit 0x{:x}", self.pos, &stack.size());
                            }
                            stack.size() as u64
                        }
                        0x14 => {
                            unimplemented!("GS:[14]  get stack canary")
                        }
                        0x1488 => {
                            if self.cfg.verbose >= 1 {
                                println!("Reading SEH 0x{:x}", self.seh);
                            }
                            self.seh
                        }
                        _ => {
                            println!("unimplemented gs:[{}]", mem_addr);
                            return None;
                        }
                    };
                    mem_addr = value;
                }

                let value: u64;
                if derref {
                    let sz = self.get_operand_sz(ins, noperand);

                    match self.hook.hook_on_memory_read {
                        Some(hook_fn) => hook_fn(self, self.regs.rip, mem_addr, sz),
                        None => (),
                    }

                    value = match sz {
                        64 => match self.maps.read_qword(mem_addr) {
                            Some(v) => v,
                            None => {
                                println!("/!\\ error dereferencing qword on 0x{:x}", mem_addr);
                                self.exception();
                                return None;
                            }
                        },

                        32 => match self.maps.read_dword(mem_addr) {
                            Some(v) => v.into(),
                            None => {
                                println!("/!\\ error dereferencing dword on 0x{:x}", mem_addr);
                                self.exception();
                                return None;
                            }
                        },

                        16 => match self.maps.read_word(mem_addr) {
                            Some(v) => v.into(),
                            None => {
                                println!("/!\\ error dereferencing word on 0x{:x}", mem_addr);
                                self.exception();
                                return None;
                            }
                        },

                        8 => match self.maps.read_byte(mem_addr) {
                            Some(v) => v.into(),
                            None => {
                                println!("/!\\ error dereferencing byte on 0x{:x}", mem_addr);
                                self.exception();
                                return None;
                            }
                        },

                        _ => unimplemented!("weird size"),
                    };

                    if self.cfg.trace_mem {
                        let name = match self.maps.get_addr_name(mem_addr) {
                            Some(n) => n,
                            None => "not mapped".to_string(),
                        };
                        println!("\tmem_trace: pos = {} rip = {:x} op = read bits = {} address = 0x{:x} value = 0x{:x} name = '{}'", self.pos, self.regs.rip, sz, mem_addr, value, name);
                    }

                    if mem_addr == self.bp.get_mem_read() {
                        println!("Memory breakpoint on read 0x{:x}", mem_addr);
                        if self.running_script {
                            self.force_break = true;
                        } else {
                            self.spawn_console();
                        }
                    }
                } else {
                    value = mem_addr;
                }
                value
            }

            _ => unimplemented!("unimplemented operand type {:?}", ins.op_kind(noperand)),
        };
        Some(value)
    }

    pub fn set_operand_value(&mut self, ins: &Instruction, noperand: u32, value: u64) -> bool {
        assert!(ins.op_count() > noperand);

        match ins.op_kind(noperand) {
            OpKind::Register => {
                if self.regs.is_fpu(ins.op_register(noperand)) {
                    self.fpu.set_streg(ins.op_register(noperand), value as f64);
                } else {
                    self.regs.set_reg(ins.op_register(noperand), value);
                }
            }

            OpKind::Memory => {
                let mut write = true;
                let mem_addr = ins
                    .virtual_address(noperand, 0, |reg, idx, _sz| {
                        if reg == Register::FS || reg == Register::GS {
                            write = false;
                            if idx == 0 {
                                if self.linux {
                                    if self.cfg.verbose > 0 {
                                        println!("writting FS[0x{:x}] = 0x{:x}", idx, value);
                                    }
                                    if value == 0x4b6c50 {
                                        self.fs.insert(0xffffffffffffffc8, 0x4b6c50);
                                    }
                                    self.fs.insert(idx as u64, value);
                                } else {
                                    if self.cfg.verbose >= 1 {
                                        println!("fs:{:x} setting SEH to 0x{:x}", idx, value);
                                    }
                                    self.seh = value;
                                }
                            } else {
                                if self.linux {
                                    if self.cfg.verbose > 0 {
                                        println!("writting FS[0x{:x}] = 0x{:x}", idx, value);
                                    }
                                    self.fs.insert(idx as u64, value);
                                } else {
                                    unimplemented!("set FS:[{}] use same logic as linux", idx);
                                }
                            }
                            Some(0)
                        } else {
                            Some(self.regs.get_reg(reg) as u64)
                        }
                    })
                    .unwrap();

                if write {
                    let sz = self.get_operand_sz(ins, noperand);

                    let value2 = match self.hook.hook_on_memory_write {
                        Some(hook_fn) => {
                            hook_fn(self, self.regs.rip, mem_addr, sz, value as u128) as u64
                        }
                        None => value,
                    };

                    match sz {
                        64 => {
                            if !self.maps.write_qword(mem_addr, value2) {
                                if self.cfg.skip_unimplemented {
                                    let map = self.maps.create_map("banzai");
                                    map.set_base(mem_addr);
                                    map.set_size(8);
                                    map.write_qword(mem_addr, value2);
                                    return true;
                                } else {
                                    println!(
                                        "/!\\ exception dereferencing bad address. 0x{:x}",
                                        mem_addr
                                    );
                                    self.exception();
                                    return false;
                                }
                            }
                        }
                        32 => {
                            if !self.maps.write_dword(mem_addr, to32!(value2)) {
                                if self.cfg.skip_unimplemented {
                                    let map = self.maps.create_map("banzai");
                                    map.set_base(mem_addr);
                                    map.set_size(4);
                                    map.write_dword(mem_addr, to32!(value2));
                                    return true;
                                } else {
                                    println!(
                                        "/!\\ exception dereferencing bad address. 0x{:x}",
                                        mem_addr
                                    );
                                    self.exception();
                                    return false;
                                }
                            }
                        }
                        16 => {
                            if !self.maps.write_word(mem_addr, value2 as u16) {
                                if self.cfg.skip_unimplemented {
                                    let map = self.maps.create_map("banzai");
                                    map.set_base(mem_addr);
                                    map.set_size(2);
                                    map.write_word(mem_addr, value2 as u16);
                                    return true;
                                } else {
                                    println!(
                                        "/!\\ exception dereferencing bad address. 0x{:x}",
                                        mem_addr
                                    );
                                    self.exception();
                                    return false;
                                }
                            }
                        }
                        8 => {
                            if !self.maps.write_byte(mem_addr, value2 as u8) {
                                if self.cfg.skip_unimplemented {
                                    let map = self.maps.create_map("banzai");
                                    map.set_base(mem_addr);
                                    map.set_size(1);
                                    map.write_byte(mem_addr, value2 as u8);
                                    return true;
                                } else {
                                    println!(
                                        "/!\\ exception dereferencing bad address. 0x{:x}",
                                        mem_addr
                                    );
                                    self.exception();
                                    return false;
                                }
                            }
                        }
                        _ => unimplemented!("weird size"),
                    }

                    if self.cfg.trace_mem {
                        let name = match self.maps.get_addr_name(mem_addr) {
                            Some(n) => n,
                            None => "not mapped".to_string(),
                        };
                        println!("\tmem_trace: pos = {} rip = {:x} op = write bits = {} address = 0x{:x} value = 0x{:x} name = '{}'", self.pos, self.regs.rip, sz, mem_addr, value2, name);
                    }

                    let name = match self.maps.get_addr_name(mem_addr) {
                        Some(n) => n,
                        None => "not mapped".to_string(),
                    };

                    if name == "code" {
                        if self.cfg.verbose >= 1 {
                            println!("/!\\ polymorfic code");
                        }
                        self.force_break = true;
                    }

                    if mem_addr == self.bp.get_mem_write() {
                        println!("Memory breakpoint on write 0x{:x}", mem_addr);
                        if self.running_script {
                            self.force_break = true;
                        } else {
                            self.spawn_console();
                        }
                    }
                }
            }

            _ => unimplemented!("unimplemented operand type"),
        };
        true
    }

    pub fn get_operand_xmm_value_128(
        &mut self,
        ins: &Instruction,
        noperand: u32,
        do_derref: bool,
    ) -> Option<u128> {
        assert!(ins.op_count() > noperand);

        let value: u128 = match ins.op_kind(noperand) {
            OpKind::Register => self.regs.get_xmm_reg(ins.op_register(noperand)),

            OpKind::Immediate64 => ins.immediate64() as u64 as u128,
            OpKind::Immediate8 => ins.immediate8() as u8 as u128,
            OpKind::Immediate16 => ins.immediate16() as u16 as u128,
            OpKind::Immediate32 => ins.immediate32() as u32 as u128,
            OpKind::Immediate8to64 => ins.immediate8to64() as u128,
            OpKind::Immediate32to64 => ins.immediate32to64() as u128,
            OpKind::Immediate8to32 => ins.immediate8to32() as u32 as u128,
            OpKind::Immediate8to16 => ins.immediate8to16() as u16 as u128,

            OpKind::Memory => {
                let mem_addr = match ins.virtual_address(noperand, 0, |reg, idx, _sz| {
                    Some(self.regs.get_reg(reg) as u64)
                }) {
                    Some(addr) => addr,
                    None => {
                        println!("/!\\ xmm exception reading operand");
                        self.exception();
                        return None;
                    }
                };

                if do_derref {
                    match self.hook.hook_on_memory_read {
                        Some(hook_fn) => hook_fn(self, self.regs.rip, mem_addr, 128),
                        None => (),
                    }

                    let value: u128 = match self.maps.read_128bits_le(mem_addr) {
                        Some(v) => v,
                        None => {
                            println!("/!\\ exception reading xmm operand at 0x{:x} ", mem_addr);
                            self.exception();
                            return None;
                        }
                    };
                    value
                } else {
                    mem_addr as u128
                }
            }
            _ => unimplemented!("unimplemented operand type {:?}", ins.op_kind(noperand)),
        };
        Some(value)
    }

    pub fn set_operand_xmm_value_128(&mut self, ins: &Instruction, noperand: u32, value: u128) {
        assert!(ins.op_count() > noperand);

        match ins.op_kind(noperand) {
            OpKind::Register => self.regs.set_xmm_reg(ins.op_register(noperand), value),
            OpKind::Memory => {
                let mem_addr = match ins.virtual_address(noperand, 0, |reg, idx, _sz| {
                    Some(self.regs.get_reg(reg) as u64)
                }) {
                    Some(addr) => addr,
                    None => {
                        println!("/!\\ exception setting xmm operand.");
                        self.exception();
                        return;
                    }
                };

                let value2 = match self.hook.hook_on_memory_write {
                    Some(hook_fn) => hook_fn(self, self.regs.rip, mem_addr, 128, value),
                    None => value,
                };

                for (i, b) in value2.to_le_bytes().iter().enumerate() {
                    self.maps.write_byte(mem_addr + i as u64, *b);
                }
            }
            _ => unimplemented!("unimplemented operand type {:?}", ins.op_kind(noperand)),
        };
    }

    pub fn get_operand_ymm_value_256(
        &mut self,
        ins: &Instruction,
        noperand: u32,
        do_derref: bool,
    ) -> Option<regs64::U256> {
        assert!(ins.op_count() > noperand);

        let value: regs64::U256 = match ins.op_kind(noperand) {
            OpKind::Register => self.regs.get_ymm_reg(ins.op_register(noperand)),

            OpKind::Immediate64 => regs64::U256::from(ins.immediate64() as u64),
            OpKind::Immediate8 => regs64::U256::from(ins.immediate8() as u8 as u64),
            OpKind::Immediate16 => regs64::U256::from(ins.immediate16() as u16 as u64),
            OpKind::Immediate32 => regs64::U256::from(ins.immediate32() as u32 as u64),
            OpKind::Immediate8to64 => regs64::U256::from(ins.immediate8to64() as u64),
            OpKind::Immediate32to64 => regs64::U256::from(ins.immediate32to64() as u64),
            OpKind::Immediate8to32 => regs64::U256::from(ins.immediate8to32() as u32 as u64),
            OpKind::Immediate8to16 => regs64::U256::from(ins.immediate8to16() as u16 as u64),

            OpKind::Memory => {
                let mem_addr = match ins.virtual_address(noperand, 0, |reg, idx, _sz| {
                    Some(self.regs.get_reg(reg) as u64)
                }) {
                    Some(addr) => addr,
                    None => {
                        println!("/!\\ xmm exception reading operand");
                        self.exception();
                        return None;
                    }
                };

                if do_derref {
                    match self.hook.hook_on_memory_read {
                        Some(hook_fn) => hook_fn(self, self.regs.rip, mem_addr, 256),
                        None => (),
                    }

                    let bytes = self.maps.read_bytes(mem_addr, 32);
                    let value = regs64::U256::from_little_endian(&bytes);

                    value
                } else {
                    regs64::U256::from(mem_addr as u64)
                }
            }
            _ => unimplemented!("unimplemented operand type {:?}", ins.op_kind(noperand)),
        };
        Some(value)
    }

    pub fn set_operand_ymm_value_256(
        &mut self,
        ins: &Instruction,
        noperand: u32,
        value: regs64::U256,
    ) {
        assert!(ins.op_count() > noperand);

        match ins.op_kind(noperand) {
            OpKind::Register => self.regs.set_ymm_reg(ins.op_register(noperand), value),
            OpKind::Memory => {
                let mem_addr = match ins.virtual_address(noperand, 0, |reg, idx, _sz| {
                    Some(self.regs.get_reg(reg) as u64)
                }) {
                    Some(addr) => addr,
                    None => {
                        println!("/!\\ exception setting xmm operand.");
                        self.exception();
                        return;
                    }
                };

                // ymm dont support value modification from hook, for now
                let value_u128: u128 = ((value.0[1] as u128) << 64) | value.0[0] as u128;
                let value2 = match self.hook.hook_on_memory_write {
                    Some(hook_fn) => hook_fn(self, self.regs.rip, mem_addr, 256, value_u128),
                    None => value_u128,
                };

                let mut bytes: Vec<u8> = vec![0; 32];
                value.to_little_endian(&mut bytes);
                self.maps.write_bytes(mem_addr, bytes);
            }
            _ => unimplemented!("unimplemented operand type {:?}", ins.op_kind(noperand)),
        };
    }

    fn get_operand_sz(&self, ins: &Instruction, noperand: u32) -> u32 {
        let reg: Register = ins.op_register(noperand);
        if reg.is_xmm() {
            return 128;
        }
        if reg.is_ymm() {
            return 256;
        }

        let size: u32 = match ins.op_kind(noperand) {
            OpKind::NearBranch64 => 64,
            OpKind::NearBranch32 => 32,
            OpKind::NearBranch16 => 16,
            OpKind::FarBranch32 => 32,
            OpKind::FarBranch16 => 16,
            OpKind::Immediate8 => 8,
            OpKind::Immediate16 => 16,
            OpKind::Immediate32 => 32,
            OpKind::Immediate64 => 64,
            OpKind::Immediate8to32 => 32,
            OpKind::Immediate8to16 => 16,
            OpKind::Immediate32to64 => 64,
            OpKind::Immediate8to64 => 64, //TODO: this could be 8
            OpKind::Register => self.regs.get_size(ins.op_register(noperand)),
            OpKind::Memory => {
                let mut info_factory = InstructionInfoFactory::new();
                let info = info_factory.info(ins);
                let mem = info.used_memory()[0];

                let size2: u32 = match mem.memory_size() {
                    MemorySize::Float16 => 16,
                    MemorySize::Float32 => 32,
                    MemorySize::Float64 => 64,
                    MemorySize::FpuEnv28 => 32,
                    MemorySize::UInt64 => 64,
                    MemorySize::UInt32 => 32,
                    MemorySize::UInt16 => 16,
                    MemorySize::UInt8 => 8,
                    MemorySize::Int64 => 64,
                    MemorySize::Int32 => 32,
                    MemorySize::Int16 => 16,
                    MemorySize::Int8 => 8,
                    MemorySize::QwordOffset => 64,
                    MemorySize::DwordOffset => 32,
                    MemorySize::WordOffset => 16,
                    MemorySize::Packed128_UInt64 => 64, // 128bits packed in 2 qwords
                    MemorySize::Packed128_UInt32 => 32, // 128bits packed in 4 dwords
                    MemorySize::Packed128_UInt16 => 16, // 128bits packed in 8 words
                    MemorySize::Bound32_DwordDword => 32,
                    MemorySize::Bound16_WordWord => 16,
                    MemorySize::Packed64_Float32 => 32,
                    MemorySize::Packed256_UInt16 => 16,
                    MemorySize::Packed256_UInt32 => 32,
                    MemorySize::Packed256_UInt64 => 64,
                    MemorySize::Packed256_UInt128 => 128,
                    MemorySize::Packed128_Float32 => 32,
                    MemorySize::SegPtr32 => 32,
                    _ => unimplemented!("memory size {:?}", mem.memory_size()),
                };

                size2
            }
            _ => unimplemented!("operand type {:?}", ins.op_kind(noperand)),
        };

        size
    }

    pub fn show_instruction(&self, color: &str, ins: &Instruction) {
        if self.cfg.verbose >= 2 {
            println!(
                "{}{} 0x{:x}: {}{}",
                color,
                self.pos,
                ins.ip(),
                self.out,
                self.colors.nc
            );
        }
    }

    pub fn show_instruction_ret(&self, color: &str, ins: &Instruction, addr: u64) {
        if self.cfg.verbose >= 2 {
            println!(
                "{}{} 0x{:x}: {} ; ret-addr: 0x{:x} ret-value: 0x{:x} {}",
                color,
                self.pos,
                ins.ip(),
                self.out,
                addr,
                self.regs.rax,
                self.colors.nc
            );
        }
    }

    pub fn show_instruction_pushpop(&self, color: &str, ins: &Instruction, value: u64) {
        if self.cfg.verbose >= 2 {
            println!(
                "{}{} 0x{:x}: {} ;0x{:x} {}",
                color,
                self.pos,
                ins.ip(),
                self.out,
                value,
                self.colors.nc
            );
        }
    }

    pub fn show_instruction_taken(&self, color: &str, ins: &Instruction) {
        if self.cfg.verbose >= 2 {
            println!(
                "{}{} 0x{:x}: {} taken {}",
                color,
                self.pos,
                ins.ip(),
                self.out,
                self.colors.nc
            );
        }
    }

    pub fn show_instruction_not_taken(&self, color: &str, ins: &Instruction) {
        if self.cfg.verbose >= 2 {
            println!(
                "{}{} 0x{:x}: {} not taken {}",
                color,
                self.pos,
                ins.ip(),
                self.out,
                self.colors.nc
            );
        }
    }

    pub fn stop(&mut self) {
        self.is_running.store(0, atomic::Ordering::Relaxed);
    }

    pub fn call32(&mut self, addr: u64, args: &[u64]) -> Result<u32, ScemuError> {
        if addr == self.regs.get_eip() {
            return Err(ScemuError::new(
                "return address reached after starting, change eip.",
            ));
        }
        let orig_stack = self.regs.get_esp();
        for arg in args.iter().rev() {
            self.stack_push32(*arg as u32);
        }
        let ret_addr = self.regs.get_eip();
        self.stack_push32(ret_addr as u32);
        self.regs.set_eip(addr);
        self.run(Some(ret_addr))?;
        self.regs.set_esp(orig_stack);
        return Ok(self.regs.get_eax() as u32);
    }

    pub fn call64(&mut self, addr: u64, args: &[u64]) -> Result<u64, ScemuError> {
        if addr == self.regs.rip {
            return Err(ScemuError::new(
                "return address reached after starting, change rip.",
            ));
        }

        let n = args.len();
        if n >= 1 {
            self.regs.rcx = args[0];
        }
        if n >= 2 {
            self.regs.rdx = args[1];
        }
        if n >= 3 {
            self.regs.r8 = args[2];
        }
        if n >= 4 {
            self.regs.r9 = args[3];
        }
        let orig_stack = self.regs.rsp;
        if n > 4 {
            for arg in args.iter().skip(4).rev() {
                self.stack_push64(*arg);
            }
        }

        let ret_addr = self.regs.rip;
        self.stack_push64(ret_addr);
        self.regs.rip = addr;
        self.run(Some(ret_addr))?;
        self.regs.rsp = orig_stack;
        return Ok(self.regs.rax);
    }

    pub fn run_until_ret(&mut self) -> Result<u64, ScemuError> {
        self.run_until_ret = true;
        return self.run(None);
    }

    pub fn capture_pre_op(&mut self) {
        self.pre_op_regs = self.regs.clone();
        self.pre_op_flags = self.flags.clone();
    }

    pub fn capture_post_op(&mut self) {
        self.post_op_regs = self.regs.clone();
        self.post_op_flags = self.flags.clone();
    }

    pub fn diff_pre_op_post_op(&mut self) {
        Regs64::diff(
            self.pre_op_regs.rip,
            self.pos - 1,
            self.pre_op_regs,
            self.post_op_regs,
        );
        Flags::diff(
            self.pre_op_regs.rip,
            self.pos - 1,
            self.pre_op_flags,
            self.post_op_flags,
        );
    }

    pub fn step(&mut self) -> bool {
        self.pos += 1;

        // code
        let code = match self.maps.get_mem_by_addr(self.regs.rip) {
            Some(c) => c,
            None => {
                println!(
                    "redirecting code flow to non maped address 0x{:x}",
                    self.regs.rip
                );
                self.spawn_console();
                return false;
            }
        };

        // block
        let block = code.read_from(self.regs.rip).to_vec(); // reduce code block for more speed
                                                            //
                                                            // decoder
        let mut decoder;
        if self.cfg.is_64bits {
            decoder = Decoder::with_ip(64, &block, self.regs.rip, DecoderOptions::NONE);
        } else {
            decoder = Decoder::with_ip(32, &block, self.regs.get_eip(), DecoderOptions::NONE);
        }

        // formatter
        let mut formatter = IntelFormatter::new();
        formatter.options_mut().set_digit_separator("");
        formatter.options_mut().set_first_operand_char_index(6);
        // get first instruction from iterator
        let ins = decoder.iter().next().unwrap();
        // size
        let sz = ins.len();

        // clear
        self.out.clear();
        formatter.format(&ins, &mut self.out);

        // emulate
        let result_ok = self.emulate_instruction(&ins, sz, true);

        // update eip/rip
        if self.force_reload {
            self.force_reload = false;
        } else {
            if self.cfg.is_64bits {
                self.regs.rip += sz as u64;
            } else {
                self.regs.set_eip(self.regs.get_eip() + sz as u64);
            }
        }

        return result_ok;
    }

    ///  RUN ENGINE ///

    pub fn run(&mut self, end_addr: Option<u64>) -> Result<u64, ScemuError> {
        self.is_running.store(1, atomic::Ordering::Relaxed);
        let is_running2 = Arc::clone(&self.is_running);

        if self.enabled_ctrlc {
            ctrlc::set_handler(move || {
                println!("Ctrl-C detected, spawning console");
                is_running2.store(0, atomic::Ordering::Relaxed);
            })
            .expect("ctrl-c handler failed");
        }

        let mut looped: Vec<u64> = Vec::new();
        let mut prev_addr: u64 = 0;
        //let mut prev_prev_addr:u64 = 0;
        let mut repeat_counter: u32 = 0;

        if end_addr.is_none() {
            println!(" ----- emulation -----");
        }

        //let ins = Instruction::default();
        let mut formatter = IntelFormatter::new();
        formatter.options_mut().set_digit_separator("");
        formatter.options_mut().set_first_operand_char_index(6);

        //self.pos = 0;

        loop {
            while self.is_running.load(atomic::Ordering::Relaxed) == 1 {
                //println!("reloading rip 0x{:x}", self.regs.rip);
                let code = match self.maps.get_mem_by_addr(self.regs.rip) {
                    Some(c) => c,
                    None => {
                        println!(
                            "redirecting code flow to non maped address 0x{:x}",
                            self.regs.rip
                        );
                        self.spawn_console();
                        return Err(ScemuError::new("cannot read program counter"));
                    }
                };
                let block = code.read_from(self.regs.rip).to_vec();
                let mut decoder;

                if self.cfg.is_64bits {
                    decoder = Decoder::with_ip(64, &block, self.regs.rip, DecoderOptions::NONE);
                } else {
                    decoder =
                        Decoder::with_ip(32, &block, self.regs.get_eip(), DecoderOptions::NONE);
                }

                for ins in decoder.iter() {
                    let sz = ins.len();
                    let addr = ins.ip();

                    if !end_addr.is_none() && Some(addr) == end_addr {
                        return Ok(self.regs.rip);
                    }

                    self.out.clear();
                    formatter.format(&ins, &mut self.out);

                    self.pos += 1;

                    if self.exp == self.pos
                        || self.pos == self.bp.get_instruction()
                        || self.bp.get_bp() == addr
                        || (self.cfg.console2 && self.cfg.console_addr == addr)
                    {
                        if self.running_script {
                            return Ok(self.regs.rip);
                        }

                        self.cfg.console2 = false;
                        println!("-------");
                        println!("{} 0x{:x}: {}", self.pos, ins.ip(), self.out);
                        self.spawn_console();
                        if self.force_break {
                            self.force_break = false;
                            break;
                        }
                    }

                    // prevent infinite loop
                    if addr == prev_addr {
                        // || addr == prev_prev_addr {
                        repeat_counter += 1;
                    }
                    //prev_prev_addr = prev_addr;
                    prev_addr = addr;
                    if repeat_counter == 100 {
                        println!("infinite loop!  opcode: {}", ins.op_code().op_code_string());
                        return Err(ScemuError::new("inifinite loop found"));
                    }

                    if self.cfg.loops {
                        // loop detector
                        looped.push(addr);
                        let mut count: u32 = 0;
                        for a in looped.iter() {
                            if addr == *a {
                                count += 1;
                            }
                        }
                        if count > 2 {
                            println!("    loop: {} interations", count);
                        }
                        /*
                        if count > self.loop_limit {
                        panic!("/!\\ iteration limit reached");
                        }*/
                        //TODO: if more than x addresses remove the bottom ones
                    }

                    if self.cfg.trace_regs {
                        if self.cfg.is_64bits {
                            self.capture_pre_op();
                            println!(
                              "\trax: 0x{:x} rbx: 0x{:x} rcx: 0x{:x} rdx: 0x{:x} rsi: 0x{:x} rdi: 0x{:x} rbp: 0x{:x} rsp: 0x{:x}",
                              self.regs.rax, self.regs.rbx, self.regs.rcx,
                              self.regs.rdx, self.regs.rsi, self.regs.rdi, self.regs.rbp, self.regs.rsp
                            );
                            // 64-bits (bytes 0-8)
                            println!(
                              "\tr8: 0x{:x} r9: 0x{:x} r10: 0x{:x} r11: 0x{:x} r12: 0x{:x} r13: 0x{:x} r14: 0x{:x} r15: 0x{:x}",
                              self.regs.r8, self.regs.r9, self.regs.r10, self.regs.r11, self.regs.r12, self.regs.r13, self.regs.r14,
                              self.regs.r15,
                            );
                            // 32-bits (upper, unofficial, bytes 4-7)
                            println!(
                              "\tr8u: 0x{:x} r9u: 0x{:x} r10u: 0x{:x} r11u: 0x{:x} r12u: 0x{:x} r13u: 0x{:x} r14u: 0x{:x} r15u: 0x{:x}",
                              self.regs.get_r8u(), self.regs.get_r9u(), self.regs.get_r10u(), self.regs.get_r11u(), self.regs.get_r12u(), self.regs.get_r13u(), self.regs.get_r14u(),
                              self.regs.get_r15u(),
                            );
                            // 32-bits (lower, bytes 0-3)
                            println!(
                              "\tr8d: 0x{:x} r9d: 0x{:x} r10d: 0x{:x} r11d: 0x{:x} r12d: 0x{:x} r13d: 0x{:x} r14d: 0x{:x} r15d: 0x{:x}",
                              self.regs.get_r8d(), self.regs.get_r9d(), self.regs.get_r10d(), self.regs.get_r11d(), self.regs.get_r12d(), self.regs.get_r13d(), self.regs.get_r14d(),
                              self.regs.get_r15d(),
                            );
                            // 16-bits (bytes 0-1)
                            println!(
                              "\tr8w: 0x{:x} r9w: 0x{:x} r10w: 0x{:x} r11w: 0x{:x} r12w: 0x{:x} r13w: 0x{:x} r14w: 0x{:x} r15w: 0x{:x}",
                              self.regs.get_r8w(), self.regs.get_r9w(), self.regs.get_r10w(), self.regs.get_r11w(), self.regs.get_r12w(), self.regs.get_r13w(), self.regs.get_r14w(),
                              self.regs.get_r15w(),
                            );
                            // 8-bits (bytes 0, should end in b and not l)
                            println!(
                              "\tr8l: 0x{:x} r9l: 0x{:x} r10l: 0x{:x} r11l: 0x{:x} r12l: 0x{:x} r13l: 0x{:x} r14l: 0x{:x} r15l: 0x{:x}",
                              self.regs.get_r8l(), self.regs.get_r9l(), self.regs.get_r10l(), self.regs.get_r11l(), self.regs.get_r12l(), self.regs.get_r13l(), self.regs.get_r14l(),
                              self.regs.get_r15l(),
                            );
                            // flags
                            println!(
                              "\tzf: {:?} pf: {:?} af: {:?} of: {:?} sf: {:?} df: {:?} cf: {:?} tf: {:?} if: {:?} nt: {:?}",
                              self.flags.f_zf, self.flags.f_pf, self.flags.f_af,
                              self.flags.f_of, self.flags.f_sf, self.flags.f_df,
                              self.flags.f_cf, self.flags.f_tf, self.flags.f_if,
                              self.flags.f_nt
                            );
                        } else {
                            // TODO: capture pre_op_registers 32-bits?
                            println!("\teax: 0x{:x} ebx: 0x{:x} ecx: 0x{:x} edx: 0x{:x} esi: 0x{:x} edi: 0x{:x} ebp: 0x{:x} esp: 0x{:x}",
                              self.regs.get_eax() as u32, self.regs.get_ebx() as u32, self.regs.get_ecx() as u32,
                              self.regs.get_edx() as u32, self.regs.get_esi() as u32, self.regs.get_edi() as u32,
                              self.regs.get_ebp() as u32, self.regs.get_esp() as u32);
                        }
                    }

                    if self.cfg.trace_reg {
                        for reg in self.cfg.reg_names.iter() {
                            match reg.as_str() {
                                "rax" => self.regs.show_rax(&self.maps, self.pos),
                                "rbx" => self.regs.show_rbx(&self.maps, self.pos),
                                "rcx" => self.regs.show_rcx(&self.maps, self.pos),
                                "rdx" => self.regs.show_rdx(&self.maps, self.pos),
                                "rsi" => self.regs.show_rsi(&self.maps, self.pos),
                                "rdi" => self.regs.show_rdi(&self.maps, self.pos),
                                "rbp" => println!("\t{} rbp: 0x{:x}", self.pos, self.regs.rbp),
                                "rsp" => println!("\t{} rsp: 0x{:x}", self.pos, self.regs.rsp),
                                "rip" => println!("\t{} rip: 0x{:x}", self.pos, self.regs.rip),
                                "r8" => self.regs.show_r8(&self.maps, self.pos),
                                "r9" => self.regs.show_r9(&self.maps, self.pos),
                                "r10" => self.regs.show_r10(&self.maps, self.pos),
                                "r10d" => self.regs.show_r10d(&self.maps, self.pos),
                                "r11" => self.regs.show_r11(&self.maps, self.pos),
                                "r11d" => self.regs.show_r11d(&self.maps, self.pos),
                                "r12" => self.regs.show_r12(&self.maps, self.pos),
                                "r13" => self.regs.show_r13(&self.maps, self.pos),
                                "r14" => self.regs.show_r14(&self.maps, self.pos),
                                "r15" => self.regs.show_r15(&self.maps, self.pos),
                                "eax" => self.regs.show_eax(&self.maps, self.pos),
                                "ebx" => self.regs.show_ebx(&self.maps, self.pos),
                                "ecx" => self.regs.show_ecx(&self.maps, self.pos),
                                "edx" => self.regs.show_edx(&self.maps, self.pos),
                                "esi" => self.regs.show_esi(&self.maps, self.pos),
                                "edi" => self.regs.show_edi(&self.maps, self.pos),
                                "esp" => println!(
                                    "\t{} esp: 0x{:x}",
                                    self.pos,
                                    self.regs.get_esp() as u32
                                ),
                                "ebp" => println!(
                                    "\t{} ebp: 0x{:x}",
                                    self.pos,
                                    self.regs.get_ebp() as u32
                                ),
                                "eip" => println!(
                                    "\t{} eip: 0x{:x}",
                                    self.pos,
                                    self.regs.get_eip() as u32
                                ),
                                "xmm1" => println!("\t{} xmm1: 0x{:x}", self.pos, self.regs.xmm1),
                                _ => panic!("invalid register."),
                            }
                        }
                    }

                    if self.cfg.trace_string {
                        let s = self.maps.read_string(self.cfg.string_addr);

                        if s.len() >= 2 && s.len() < 80 {
                            println!("\ttrace string -> 0x{:x}: '{}'", self.cfg.string_addr, s);
                        } else {
                            let w = self.maps.read_wide_string(self.cfg.string_addr);
                            if w.len() < 80 {
                                println!(
                                    "\ttrace wide string -> 0x{:x}: '{}'",
                                    self.cfg.string_addr, w
                                );
                            } else {
                                println!("\ttrace wide string -> 0x{:x}: ''", self.cfg.string_addr);
                            }
                        }
                    }

                    //let mut info_factory = InstructionInfoFactory::new();
                    //let info = info_factory.info(&ins);

                    match self.hook.hook_on_pre_instruction {
                        Some(hook_fn) => hook_fn(self, self.regs.rip, &ins, sz),
                        None => (),
                    }

                    let emulation_ok = self.emulate_instruction(&ins, sz, false);

                    match self.hook.hook_on_post_instruction {
                        Some(hook_fn) => hook_fn(self, self.regs.rip, &ins, sz, emulation_ok),
                        None => (),
                    }

                    if self.cfg.inspect {
                        let addr: u64 =
                            self.memory_operand_to_address(self.cfg.inspect_seq.clone().as_str());
                        let bits = self.get_size(self.cfg.inspect_seq.clone().as_str());
                        let value = self
                            .memory_read(self.cfg.inspect_seq.clone().as_str())
                            .unwrap_or(0);
                        println!(
                            "\tmem_inspect: rip = {:x} (0x{:x}): 0x{:x} {} '{}' {{{}}}",
                            self.regs.rip,
                            addr,
                            value,
                            value,
                            self.maps.read_string(addr),
                            self.maps
                                .read_string_of_bytes(addr, constants::NUM_BYTES_TRACE)
                        );
                    }

                    if self.cfg.trace_regs {
                        // registers
                        if self.cfg.is_64bits {
                            self.capture_post_op();
                            self.diff_pre_op_post_op();
                        } else {
                            // TODO: self.diff_pre_op_post_op_registers_32bits();
                        }
                    }

                    if !emulation_ok {
                        if self.cfg.console_enabled {
                            self.spawn_console();
                        } else {
                            return Err(ScemuError::new("emulation error"));
                        }
                    }

                    if self.force_reload {
                        self.force_reload = false;
                        break;
                    }

                    if self.cfg.is_64bits {
                        self.regs.rip += sz as u64;
                    } else {
                        self.regs.set_eip(self.regs.get_eip() + sz as u64);
                    }

                    if self.force_break {
                        self.force_break = false;
                        break;
                    }
                } // end decoder loop
            } // end running loop

            self.is_running.store(1, atomic::Ordering::Relaxed);
            self.spawn_console();
        } // end infinite loop
    } // end run

    //////////// EMULATE INSTRUCTION ////////////

    fn emulate_instruction(
        &mut self,
        ins: &Instruction,
        instruction_sz: usize,
        rep_step: bool,
    ) -> bool {
        match ins.mnemonic() {
            Mnemonic::Jmp => {
                self.show_instruction(&self.colors.yellow, &ins);

                if ins.op_count() != 1 {
                    unimplemented!("weird variant of jmp");
                }

                let addr = match self.get_operand_value(&ins, 0, true) {
                    Some(a) => a,
                    None => return false,
                };

                if self.cfg.is_64bits {
                    return self.set_rip(addr, false);
                } else {
                    return self.set_eip(addr, false);
                }
            }

            Mnemonic::Call => {
                self.show_instruction(&self.colors.yellow, &ins);

                if ins.op_count() != 1 {
                    unimplemented!("weird variant of call");
                }

                let addr = match self.get_operand_value(&ins, 0, true) {
                    Some(a) => a,
                    None => return false,
                };

                if self.cfg.is_64bits {
                    if !self.stack_push64(self.regs.rip + instruction_sz as u64) {
                        return false;
                    }
                    return self.set_rip(addr, false);
                } else {
                    if !self.stack_push32(self.regs.get_eip() as u32 + instruction_sz as u32) {
                        return false;
                    }
                    return self.set_eip(addr, false);
                }
            }

            Mnemonic::Push => {
                let value = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                self.show_instruction_pushpop(&self.colors.blue, &ins, value);

                if self.cfg.is_64bits {
                    if !self.stack_push64(value) {
                        return false;
                    }
                } else {
                    if !self.stack_push32(to32!(value)) {
                        return false;
                    }
                }
            }

            Mnemonic::Pop => {
                let value: u64;

                if self.cfg.is_64bits {
                    value = match self.stack_pop64(true) {
                        Some(v) => v as u64,
                        None => return false,
                    };
                } else {
                    value = match self.stack_pop32(true) {
                        Some(v) => v as u64,
                        None => return false,
                    };
                }

                self.show_instruction_pushpop(&self.colors.blue, &ins, value);

                if !self.set_operand_value(&ins, 0, value) {
                    return false;
                }
            }

            Mnemonic::Pushad => {
                self.show_instruction(&self.colors.blue, &ins);

                // only 32bits instruction
                let tmp_esp = self.regs.get_esp() as u32;
                if !self.stack_push32(self.regs.get_eax() as u32) {
                    return false;
                }
                if !self.stack_push32(self.regs.get_ecx() as u32) {
                    return false;
                }
                if !self.stack_push32(self.regs.get_edx() as u32) {
                    return false;
                }
                if !self.stack_push32(self.regs.get_ebx() as u32) {
                    return false;
                }
                if !self.stack_push32(tmp_esp) {
                    return false;
                }
                if !self.stack_push32(self.regs.get_ebp() as u32) {
                    return false;
                }
                if !self.stack_push32(self.regs.get_esi() as u32) {
                    return false;
                }
                if !self.stack_push32(self.regs.get_edi() as u32) {
                    return false;
                }
            }

            Mnemonic::Popad => {
                self.show_instruction(&self.colors.blue, &ins);
                let mut poped: u64;

                // only 32bits instruction
                poped = self.stack_pop32(false).unwrap_or(0) as u64;
                self.regs.set_edi(poped);
                poped = self.stack_pop32(false).unwrap_or(0) as u64;
                self.regs.set_esi(poped);
                poped = self.stack_pop32(false).unwrap_or(0) as u64;
                self.regs.set_ebp(poped);

                self.regs.set_esp(self.regs.get_esp() + 4); // skip esp

                poped = self.stack_pop32(false).unwrap_or(0) as u64;
                self.regs.set_ebx(poped);
                poped = self.stack_pop32(false).unwrap_or(0) as u64;
                self.regs.set_edx(poped);
                poped = self.stack_pop32(false).unwrap_or(0) as u64;
                self.regs.set_ecx(poped);
                poped = self.stack_pop32(false).unwrap_or(0) as u64;
                self.regs.set_eax(poped);
            }

            Mnemonic::Cdqe => {
                self.show_instruction(&self.colors.blue, &ins);

                self.regs.rax = self.regs.get_eax() as u32 as i32 as i64 as u64;
                // sign extend
            }

            Mnemonic::Cdq => {
                self.show_instruction(&self.colors.blue, &ins);

                let num: i64 = self.regs.get_eax() as u32 as i32 as i64; // sign-extend
                let unum: u64 = num as u64;
                self.regs.set_edx((unum & 0xffffffff00000000) >> 32);
                // preserve upper 64-bits from getting overriden
                let rax_upper = self.regs.rax >> 32;
                self.regs.rax = (rax_upper << 32) | (unum & 0xffffffff);
            }

            Mnemonic::Cqo => {
                self.show_instruction(&self.colors.blue, &ins);

                let sigextend: u128 = self.regs.rax as u64 as i64 as i128 as u128;
                self.regs.rdx = ((sigextend & 0xffffffff_ffffffff_00000000_00000000) >> 64) as u64
            }

            Mnemonic::Ret => {
                let ret_addr: u64;

                if self.cfg.is_64bits {
                    ret_addr = match self.stack_pop64(false) {
                        Some(v) => v as u64,
                        None => return false,
                    };
                } else {
                    ret_addr = match self.stack_pop32(false) {
                        Some(v) => v as u64,
                        None => return false,
                    };
                }

                self.show_instruction_ret(&self.colors.yellow, &ins, ret_addr);

                if self.run_until_ret {
                    return true; //TODO: fix this
                }

                if self.break_on_next_return {
                    self.break_on_next_return = false;
                    self.spawn_console();
                }

                if ins.op_count() > 0 {
                    let mut arg = self
                        .get_operand_value(&ins, 0, true)
                        .expect("weird crash on ret");
                    // apply stack compensation of ret operand

                    if self.cfg.is_64bits {
                        if arg % 8 != 0 {
                            panic!("weird ret argument!");
                        }

                        arg /= 8;

                        for _ in 0..arg {
                            self.stack_pop64(false);
                        }
                    } else {
                        if arg % 4 != 0 {
                            println!("weird ret argument!");
                            return false;
                        }

                        arg /= 4;

                        for _ in 0..arg {
                            self.stack_pop32(false);
                        }
                    }
                }

                if self.eh_ctx != 0 {
                    exception::exit(self);
                    return true;
                }

                if self.cfg.is_64bits {
                    return self.set_rip(ret_addr, false);
                } else {
                    return self.set_eip(ret_addr, false);
                }
            }

            Mnemonic::Xchg => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                assert!(ins.op_count() == 2);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                if !self.set_operand_value(&ins, 0, value1) {
                    return false;
                }
                if !self.set_operand_value(&ins, 1, value0) {
                    return false;
                }
            }

            Mnemonic::Aad => {
                self.show_instruction(&self.colors.light_cyan, &ins);
                assert!(ins.op_count() <= 1);

                let mut low: u64 = self.regs.get_al();
                let high: u64 = self.regs.get_ah();
                let imm: u64;

                if ins.op_count() == 0 {
                    imm = 10;
                } else {
                    imm = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };
                }

                low = (low + (imm * high)) & 0xff;
                self.regs.set_al(low);
                self.regs.set_ah(0);

                self.flags.calc_flags(low, 8);
            }

            Mnemonic::Les => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                assert!(ins.op_count() == 2);

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                if !self.set_operand_value(&ins, 0, value1) {
                    return false;
                }
            }

            Mnemonic::Mov => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                assert!(ins.op_count() == 2);

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                if !self.set_operand_value(&ins, 0, value1) {
                    return false;
                }
            }

            Mnemonic::Xor => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 2);
                assert!(self.get_operand_sz(&ins, 0) == self.get_operand_sz(&ins, 1));

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);
                let result = value0 ^ value1;

                if self.cfg.test_mode {
                    if result != inline::xor(value0, value1) {
                        panic!(
                            "0x{:x} should be 0x{:x}",
                            result,
                            inline::xor(value0, value1)
                        );
                    }
                }

                self.flags.calc_flags(result, sz);
                self.flags.f_of = false;
                self.flags.f_cf = false;
                self.flags.calc_pf(result as u8);

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Add => {
                self.show_instruction(&self.colors.cyan, &ins);

                assert!(ins.op_count() == 2);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let res: u64 = match self.get_operand_sz(&ins, 1) {
                    64 => self.flags.add64(value0, value1),
                    32 => self.flags.add32(value0, value1),
                    16 => self.flags.add16(value0, value1),
                    8 => self.flags.add8(value0, value1),
                    _ => unreachable!("weird size"),
                };

                if !self.set_operand_value(&ins, 0, res) {
                    return false;
                }
            }

            Mnemonic::Adc => {
                self.show_instruction(&self.colors.cyan, &ins);

                assert!(ins.op_count() == 2);

                let cf: u64;
                if self.flags.f_cf {
                    cf = 1
                } else {
                    cf = 0;
                }

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let res: u64;
                match self.get_operand_sz(&ins, 1) {
                    64 => res = self.flags.add64(value0, value1 + cf),
                    32 => res = self.flags.add32(value0, value1 + cf),
                    16 => res = self.flags.add16(value0, value1 + cf),
                    8 => res = self.flags.add8(value0, value1 + cf),
                    _ => unreachable!("weird size"),
                }

                if !self.set_operand_value(&ins, 0, res) {
                    return false;
                }
            }

            Mnemonic::Sbb => {
                self.show_instruction(&self.colors.cyan, &ins);

                assert!(ins.op_count() == 2);

                let cf: u64;
                if self.flags.f_cf {
                    cf = 1;
                } else {
                    cf = 0;
                }

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let res: u64;
                let sz = self.get_operand_sz(&ins, 1);
                match sz {
                    64 => res = self.flags.sub64(value0, value1 + cf),
                    32 => res = self.flags.sub32(value0, value1 + cf),
                    16 => res = self.flags.sub16(value0, value1 + cf),
                    8 => res = self.flags.sub8(value0, value1 + cf),
                    _ => panic!("weird size"),
                }

                if !self.set_operand_value(&ins, 0, res) {
                    return false;
                }
            }

            Mnemonic::Sub => {
                self.show_instruction(&self.colors.cyan, &ins);

                assert!(ins.op_count() == 2);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let res: u64;
                match self.get_operand_sz(&ins, 0) {
                    64 => res = self.flags.sub64(value0, value1),
                    32 => res = self.flags.sub32(value0, value1),
                    16 => res = self.flags.sub16(value0, value1),
                    8 => res = self.flags.sub8(value0, value1),
                    _ => panic!("weird size"),
                }

                if !self.set_operand_value(&ins, 0, res) {
                    return false;
                }
            }

            Mnemonic::Inc => {
                self.show_instruction(&self.colors.cyan, &ins);

                assert!(ins.op_count() == 1);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let res = match self.get_operand_sz(&ins, 0) {
                    64 => self.flags.inc64(value0),
                    32 => self.flags.inc32(value0),
                    16 => self.flags.inc16(value0),
                    8 => self.flags.inc8(value0),
                    _ => panic!("weird size"),
                };

                if !self.set_operand_value(&ins, 0, res) {
                    return false;
                }
            }

            Mnemonic::Dec => {
                self.show_instruction(&self.colors.cyan, &ins);

                assert!(ins.op_count() == 1);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let res = match self.get_operand_sz(&ins, 0) {
                    64 => self.flags.dec64(value0),
                    32 => self.flags.dec32(value0),
                    16 => self.flags.dec16(value0),
                    8 => self.flags.dec8(value0),
                    _ => panic!("weird size"),
                };

                if !self.set_operand_value(&ins, 0, res) {
                    return false;
                }
            }

            Mnemonic::Neg => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 1);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);
                let res = match sz {
                    64 => self.flags.neg64(value0),
                    32 => self.flags.neg32(value0),
                    16 => self.flags.neg16(value0),
                    8 => self.flags.neg8(value0),
                    _ => panic!("weird size"),
                };

                if self.cfg.test_mode {
                    if res != inline::neg(value0, sz) {
                        panic!("0x{:x} should be 0x{:x}", res, inline::neg(value0, sz));
                    }
                }

                if value0 == 0 {
                    self.flags.f_cf = false;
                } else {
                    self.flags.f_cf = true;
                }

                if ((res | value0) & 0x8) != 0 {
                    self.flags.f_af = true;
                } else {
                    self.flags.f_af = false;
                }

                if !self.set_operand_value(&ins, 0, res) {
                    return false;
                }
            }

            Mnemonic::Not => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 1);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let val: u64;

                /*let mut ival = value0 as i32;
                ival = !ival;*/

                let sz = self.get_operand_sz(&ins, 0);
                match sz {
                    64 => {
                        let mut ival = value0 as i64;
                        ival = !ival;
                        val = ival as u64;
                    }
                    32 => {
                        let mut ival = value0 as u32 as i32;
                        ival = !ival;
                        //val = value0 & 0xffffffff_00000000 | ival as u32 as u64;
                        val = ival as u32 as u64;
                    }
                    16 => {
                        let mut ival = value0 as u16 as i16;
                        ival = !ival;
                        val = value0 & 0xffffffff_ffff0000 | ival as u16 as u64;
                    }
                    8 => {
                        let mut ival = value0 as u8 as i8;
                        ival = !ival;
                        val = value0 & 0xffffffff_ffffff00 | ival as u8 as u64;
                    }
                    _ => unimplemented!("weird"),
                }

                if self.cfg.test_mode {
                    if val != inline::not(value0, sz) {
                        panic!("0x{:x} should be 0x{:x}", val, inline::not(value0, sz));
                    }
                }

                if !self.set_operand_value(&ins, 0, val) {
                    return false;
                }
            }

            Mnemonic::And => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 2);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);
                let result1: u64;
                let result2: u64;

                match sz {
                    8 => {
                        result1 = (value0 & 0xff) & (value1 & 0xff);
                        result2 = (value0 & 0xffffffffffffff00) + result1;
                    }
                    16 => {
                        result1 = (value0 & 0xffff) & (value1 & 0xffff);
                        result2 = (value0 & 0xffffffffffff0000) + result1;
                    }
                    32 => {
                        result1 = (value0 & 0xffffffff) & (value1 & 0xffffffff);
                        result2 = (value0 & 0xffffffff00000000) + result1;
                    }
                    64 => {
                        result1 = value0 & value1;
                        result2 = result1;
                    }
                    _ => unreachable!(""),
                }

                if self.cfg.test_mode {
                    if result2 != inline::and(value0, value1) {
                        panic!(
                            "0x{:x} should be 0x{:x}",
                            result2,
                            inline::and(value0, value1)
                        );
                    }
                }

                self.flags.calc_flags(result1, self.get_operand_sz(&ins, 0));
                self.flags.f_of = false;
                self.flags.f_cf = false;
                self.flags.calc_pf(result1 as u8);

                if !self.set_operand_value(&ins, 0, result2) {
                    return false;
                }
            }

            Mnemonic::Or => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 2);
                assert!(self.get_operand_sz(&ins, 0) == self.get_operand_sz(&ins, 1));

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);
                let result1: u64;
                let result2: u64;

                match sz {
                    8 => {
                        result1 = (value0 & 0xff) | (value1 & 0xff);
                        result2 = (value0 & 0xffffffffffffff00) + result1;
                    }
                    16 => {
                        result1 = (value0 & 0xffff) | (value1 & 0xffff);
                        result2 = (value0 & 0xffffffffffff0000) + result1;
                    }
                    32 => {
                        result1 = (value0 & 0xffffffff) | (value1 & 0xffffffff);
                        result2 = (value0 & 0xffffffff00000000) + result1;
                    }
                    64 => {
                        result1 = value0 | value1;
                        result2 = result1;
                    }
                    _ => unreachable!(""),
                }

                if self.cfg.test_mode {
                    if result2 != inline::or(value0, value1) {
                        panic!(
                            "0x{:x} should be 0x{:x}",
                            result2,
                            inline::or(value0, value1)
                        );
                    }
                }

                self.flags.calc_flags(result1, self.get_operand_sz(&ins, 0));
                self.flags.f_of = false;
                self.flags.f_cf = false;
                self.flags.calc_pf(result1 as u8);

                if !self.set_operand_value(&ins, 0, result2) {
                    return false;
                }
            }

            Mnemonic::Sal => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 1 || ins.op_count() == 2);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                if ins.op_count() == 1 {
                    // 1 param

                    let sz = self.get_operand_sz(&ins, 0);
                    let result = match sz {
                        64 => self.flags.sal1p64(value0),
                        32 => self.flags.sal1p32(value0),
                        16 => self.flags.sal1p16(value0),
                        8 => self.flags.sal1p8(value0),
                        _ => panic!("weird size"),
                    };

                    if self.cfg.test_mode {
                        if result != inline::sal(value0, 1, sz) {
                            panic!(
                                "sal1p 0x{:x} should be 0x{:x}",
                                result,
                                inline::sal(value0, 1, sz)
                            );
                        }
                    }

                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                } else {
                    // 2 params

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let sz = self.get_operand_sz(&ins, 0);
                    let result = match sz {
                        64 => self.flags.sal2p64(value0, value1),
                        32 => self.flags.sal2p32(value0, value1),
                        16 => self.flags.sal2p16(value0, value1),
                        8 => self.flags.sal2p8(value0, value1),
                        _ => panic!("weird size"),
                    };

                    if self.cfg.test_mode {
                        if result != inline::sal(value0, value1, sz) {
                            panic!(
                                "sal1p 0x{:x} should be 0x{:x}",
                                result,
                                inline::sal(value0, value1, sz)
                            );
                        }
                    }

                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                }
            }

            Mnemonic::Sar => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 1 || ins.op_count() == 2);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                if ins.op_count() == 1 {
                    // 1 param

                    let sz = self.get_operand_sz(&ins, 0);
                    let result = match sz {
                        64 => self.flags.sar1p64(value0),
                        32 => self.flags.sar1p32(value0),
                        16 => self.flags.sar1p16(value0),
                        8 => self.flags.sar1p8(value0),
                        _ => panic!("weird size"),
                    };

                    if self.cfg.test_mode {
                        if result != inline::sar1p(value0, sz, self.flags.f_cf) {
                            panic!(
                                "0x{:x} should be 0x{:x}",
                                result,
                                inline::sar1p(value0, sz, self.flags.f_cf)
                            );
                        }
                    }

                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                } else {
                    // 2 params

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let sz = self.get_operand_sz(&ins, 0);
                    let result = match sz {
                        64 => self.flags.sar2p64(value0, value1),
                        32 => self.flags.sar2p32(value0, value1),
                        16 => self.flags.sar2p16(value0, value1),
                        8 => self.flags.sar2p8(value0, value1),
                        _ => panic!("weird size"),
                    };

                    if self.cfg.test_mode {
                        if result != inline::sar2p(value0, value1, sz, self.flags.f_cf) {
                            panic!(
                                "0x{:x} should be 0x{:x}",
                                result,
                                inline::sar2p(value0, value1, sz, self.flags.f_cf)
                            );
                        }
                    }

                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                }
            }

            Mnemonic::Shl => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 1 || ins.op_count() == 2);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                if ins.op_count() == 1 {
                    // 1 param

                    let sz = self.get_operand_sz(&ins, 0);
                    let result = match sz {
                        64 => self.flags.shl1p64(value0),
                        32 => self.flags.shl1p32(value0),
                        16 => self.flags.shl1p16(value0),
                        8 => self.flags.shl1p8(value0),
                        _ => panic!("weird size"),
                    };

                    if self.cfg.test_mode {
                        if result != inline::shl(value0, 1, sz) {
                            panic!(
                                "SHL 0x{:x} should be 0x{:x}",
                                result,
                                inline::shl(value0, 1, sz)
                            );
                        }
                    }

                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                } else {
                    // 2 params

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let sz = self.get_operand_sz(&ins, 0);
                    let result = match sz {
                        64 => self.flags.shl2p64(value0, value1),
                        32 => self.flags.shl2p32(value0, value1),
                        16 => self.flags.shl2p16(value0, value1),
                        8 => self.flags.shl2p8(value0, value1),
                        _ => panic!("weird size"),
                    };

                    if self.cfg.test_mode {
                        if result != inline::shl(value0, value1, sz) {
                            panic!(
                                "SHL 0x{:x} should be 0x{:x}",
                                result,
                                inline::shl(value0, value1, sz)
                            );
                        }
                    }

                    //println!("0x{:x}: 0x{:x} SHL 0x{:x} = 0x{:x}", ins.ip32(), value0, value1, result);

                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                }
            }

            Mnemonic::Shr => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 1 || ins.op_count() == 2);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                if ins.op_count() == 1 {
                    // 1 param

                    let sz = self.get_operand_sz(&ins, 0);
                    let result = match sz {
                        64 => self.flags.shr1p64(value0),
                        32 => self.flags.shr1p32(value0),
                        16 => self.flags.shr1p16(value0),
                        8 => self.flags.shr1p8(value0),
                        _ => panic!("weird size"),
                    };

                    if self.cfg.test_mode {
                        if result != inline::shr(value0, 1, sz) {
                            panic!(
                                "SHR 0x{:x} should be 0x{:x}",
                                result,
                                inline::shr(value0, 1, sz)
                            );
                        }
                    }

                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                } else {
                    // 2 params

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let sz = self.get_operand_sz(&ins, 0);
                    let result = match sz {
                        64 => self.flags.shr2p64(value0, value1),
                        32 => self.flags.shr2p32(value0, value1),
                        16 => self.flags.shr2p16(value0, value1),
                        8 => self.flags.shr2p8(value0, value1),
                        _ => panic!("weird size"),
                    };

                    if self.cfg.test_mode {
                        if result != inline::shr(value0, value1, sz) {
                            panic!(
                                "SHR 0x{:x} should be 0x{:x}",
                                result,
                                inline::shr(value0, value1, sz)
                            );
                        }
                    }

                    //println!("0x{:x} SHR 0x{:x} >> 0x{:x} = 0x{:x}", ins.ip32(), value0, value1, result);

                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                }
            }

            Mnemonic::Ror => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 1 || ins.op_count() == 2);

                let result: u64;
                let sz = self.get_operand_sz(&ins, 0);

                if ins.op_count() == 1 {
                    // 1 param
                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    result = self.ror(value0, 1, sz);
                    self.flags.calc_flags(result, sz);

                    if self.cfg.test_mode {
                        if result != inline::ror(value0, 1, sz) {
                            panic!(
                                "0x{:x} should be 0x{:x}",
                                result,
                                inline::ror(value0, 1, sz)
                            )
                        }
                    }
                } else {
                    // 2 params
                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    result = self.ror(value0, value1, sz);

                    if self.cfg.test_mode {
                        if result != inline::ror(value0, value1, sz) {
                            panic!(
                                "0x{:x} should be 0x{:x}",
                                result,
                                inline::ror(value0, value1, sz)
                            )
                        }
                    }

                    let masked_counter;
                    if sz == 64 {
                        masked_counter = value1 & 0b111111;
                    } else {
                        masked_counter = value1 & 0b11111;
                    }

                    if masked_counter > 0 {
                        if masked_counter == 1 {
                            // the OF flag is set to the exclusive OR of the two most-significant bits of the result.
                            let of = match sz {
                                64 => (result >> 62) ^ ((result >> 63) & 0b1),
                                32 => (result >> 31) ^ ((result >> 30) & 0b1),
                                16 => (result >> 15) ^ ((result >> 14) & 0b1),
                                8 => (result >> 7) ^ ((result >> 6) & 0b1),
                                _ => panic!("weird size"),
                            };
                            self.flags.f_of = of == 1;
                        } else {
                            // OF flag is undefined?
                        }
                    }
                }

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Rcr => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 1 || ins.op_count() == 2);

                let result: u64;
                let sz = self.get_operand_sz(&ins, 0);

                if ins.op_count() == 1 {
                    // 1 param
                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    result = self.rcr(value0, 1, sz);
                    self.flags.rcr_of_and_cf(value0, 1, sz);
                    self.flags.calc_flags(result, sz);
                } else {
                    // 2 params
                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    result = self.rcr(value0, value1, sz);
                    self.flags.rcr_of_and_cf(value0, value1, sz);

                    let masked_counter;
                    if sz == 64 {
                        masked_counter = value1 & 0b111111;
                    } else {
                        masked_counter = value1 & 0b11111;
                    }

                    if masked_counter > 0 {
                        self.flags.calc_flags(result, sz);
                    }
                }

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Rol => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 1 || ins.op_count() == 2);

                let result: u64;
                let sz = self.get_operand_sz(&ins, 0);

                if ins.op_count() == 1 {
                    // 1 param
                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    result = self.rol(value0, 1, sz);

                    if self.cfg.test_mode {
                        if result != inline::rol(value0, 1, sz) {
                            panic!(
                                "0x{:x} should be 0x{:x}",
                                result,
                                inline::rol(value0, 1, sz)
                            );
                        }
                    }

                    self.flags.calc_flags(result, sz);
                } else {
                    // 2 params
                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let pre_cf;
                    if self.flags.f_cf {
                        pre_cf = 1;
                    } else {
                        pre_cf = 0;
                    }

                    result = self.rol(value0, value1, sz);

                    if self.cfg.test_mode {
                        if result != inline::rol(value0, value1, sz) {
                            panic!(
                                "0x{:x} should be 0x{:x}",
                                result,
                                inline::rol(value0, value1, sz)
                            );
                        }
                    }

                    let masked_counter;
                    if sz == 64 {
                        masked_counter = value1 & 0b111111;
                    } else {
                        masked_counter = value1 & 0b11111;
                    }

                    // If the masked count is 0, the flags are not affected.
                    // If the masked count is 1, then the OF flag is affected, otherwise (masked count is greater than 1) the OF flag is undefined.
                    // The CF flag is affected when the masked count is nonzero.
                    // The SF, ZF, AF, and PF flags are always unaffected.
                    if masked_counter > 0 {
                        if masked_counter == 1 {
                            // the OF flag is set to the exclusive OR of the two most-significant bits of the result.
                            let of = match sz {
                                64 => (result >> 62) ^ pre_cf,
                                32 => (result >> 31) ^ pre_cf,
                                16 => (result >> 15) ^ pre_cf,
                                8 => (result >> 7) ^ pre_cf,
                                _ => panic!("weird size"),
                            };
                            self.flags.f_of = of == 1;
                        } else {
                            // OF flag is undefined?
                        }
                    }
                }

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Rcl => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 1 || ins.op_count() == 2);

                let result: u64;
                let sz = self.get_operand_sz(&ins, 0);

                if ins.op_count() == 1 {
                    // 1 param
                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    result = self.rcl(value0, 1, sz);
                    self.flags.calc_flags(result, sz);
                } else {
                    // 2 params
                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    result = self.rcl(value0, value1, sz);

                    let masked_counter;
                    if sz == 64 {
                        masked_counter = value1 & 0b111111;
                    } else {
                        masked_counter = value1 & 0b11111;
                    }

                    if masked_counter > 0 {
                        self.flags.calc_flags(result, sz);
                    }
                }

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Mul => {
                self.show_instruction(&self.colors.cyan, &ins);

                assert!(ins.op_count() == 1);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let pre_rax = self.regs.rax;
                let pre_rdx = self.regs.rdx;

                let sz = self.get_operand_sz(&ins, 0);
                match sz {
                    64 => self.mul64(value0),
                    32 => self.mul32(value0),
                    16 => self.mul16(value0),
                    8 => self.mul8(value0),
                    _ => unimplemented!("wrong size"),
                }

                if self.cfg.test_mode {
                    let (post_rdx, post_rax) = inline::mul(value0, pre_rax, pre_rdx, sz);
                    if post_rax != self.regs.rax || post_rdx != self.regs.rdx {
                        println!(
                            "sz: {} value0: 0x{:x} pre_rax: 0x{:x} pre_rdx: 0x{:x}",
                            sz, value0, pre_rax, pre_rdx
                        );
                        println!(
                            "mul rax is 0x{:x} and should be 0x{:x}",
                            self.regs.rax, post_rax
                        );
                        println!(
                            "mul rdx is 0x{:x} and should be 0x{:x}",
                            self.regs.rdx, post_rdx
                        );
                        panic!("inline asm test failed");
                    }
                }
            }

            Mnemonic::Div => {
                self.show_instruction(&self.colors.cyan, &ins);

                assert!(ins.op_count() == 1);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let pre_rax = self.regs.rax;
                let pre_rdx = self.regs.rdx;

                let sz = self.get_operand_sz(&ins, 0);
                match sz {
                    64 => self.div64(value0),
                    32 => self.div32(value0),
                    16 => self.div16(value0),
                    8 => self.div8(value0),
                    _ => unimplemented!("wrong size"),
                }

                if self.cfg.test_mode {
                    let (post_rdx, post_rax) = inline::div(value0, pre_rax, pre_rdx, sz);
                    if post_rax != self.regs.rax || post_rdx != self.regs.rdx {
                        println!("pos: {}", self.pos);
                        println!(
                            "sz: {} value0: 0x{:x} pre_rax: 0x{:x} pre_rdx: 0x{:x}",
                            sz, value0, pre_rax, pre_rdx
                        );
                        println!(
                            "div{} rax is 0x{:x} and should be 0x{:x}",
                            sz, self.regs.rax, post_rax
                        );
                        println!(
                            "div{} rdx is 0x{:x} and should be 0x{:x}",
                            sz, self.regs.rdx, post_rdx
                        );
                        panic!("inline asm test failed");
                    }
                }
            }

            Mnemonic::Idiv => {
                self.show_instruction(&self.colors.cyan, &ins);

                assert!(ins.op_count() == 1);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let pre_rax = self.regs.rax;
                let pre_rdx = self.regs.rdx;

                let sz = self.get_operand_sz(&ins, 0);
                match sz {
                    64 => self.idiv64(value0),
                    32 => self.idiv32(value0),
                    16 => self.idiv16(value0),
                    8 => self.idiv8(value0),
                    _ => unimplemented!("wrong size"),
                }

                if self.cfg.test_mode {
                    let (post_rdx, post_rax) = inline::idiv(value0, pre_rax, pre_rdx, sz);
                    if post_rax != self.regs.rax || post_rdx != self.regs.rdx {
                        println!(
                            "sz: {} value0: 0x{:x} pre_rax: 0x{:x} pre_rdx: 0x{:x}",
                            sz, value0, pre_rax, pre_rdx
                        );
                        println!(
                            "idiv rax is 0x{:x} and should be 0x{:x}",
                            self.regs.rax, post_rax
                        );
                        println!(
                            "idiv rdx is 0x{:x} and should be 0x{:x}",
                            self.regs.rdx, post_rdx
                        );
                        panic!("inline asm test failed");
                    }
                }
            }

            Mnemonic::Imul => {
                self.show_instruction(&self.colors.cyan, &ins);

                assert!(ins.op_count() == 1 || ins.op_count() == 2 || ins.op_count() == 3);

                if ins.op_count() == 1 {
                    // 1 param

                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let pre_rax = self.regs.rax;
                    let pre_rdx = self.regs.rdx;

                    let sz = self.get_operand_sz(&ins, 0);
                    match sz {
                        64 => self.imul64p1(value0),
                        32 => self.imul32p1(value0),
                        16 => self.imul16p1(value0),
                        8 => self.imul8p1(value0),
                        _ => unimplemented!("wrong size"),
                    }

                    if self.cfg.test_mode {
                        let (post_rdx, post_rax) = inline::imul1p(value0, pre_rax, pre_rdx, sz);
                        if post_rax != self.regs.rax || post_rdx != self.regs.rdx {
                            println!(
                                "sz: {} value0: 0x{:x} pre_rax: 0x{:x} pre_rdx: 0x{:x}",
                                sz, value0, pre_rax, pre_rdx
                            );
                            println!(
                                "imul1p rax is 0x{:x} and should be 0x{:x}",
                                self.regs.rax, post_rax
                            );
                            println!(
                                "imul1p rdx is 0x{:x} and should be 0x{:x}",
                                self.regs.rdx, post_rdx
                            );
                            panic!("inline asm test failed");
                        }
                    }
                } else if ins.op_count() == 2 {
                    // 2 params
                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let sz = self.get_operand_sz(&ins, 0);
                    let result = match sz {
                        64 => self.flags.imul64p2(value0, value1),
                        32 => self.flags.imul32p2(value0, value1),
                        16 => self.flags.imul16p2(value0, value1),
                        8 => self.flags.imul8p2(value0, value1),
                        _ => unimplemented!("wrong size"),
                    };

                    if self.cfg.test_mode {
                        if result != inline::imul2p(value0, value1, sz) {
                            panic!(
                                "imul{}p2 gives 0x{:x} and should be 0x{:x}",
                                sz,
                                result,
                                inline::imul2p(value0, value1, sz)
                            );
                        }
                    }

                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                } else {
                    // 3 params

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let value2 = match self.get_operand_value(&ins, 2, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let sz = self.get_operand_sz(&ins, 0);
                    let result = match sz {
                        64 => self.flags.imul64p2(value1, value2),
                        32 => self.flags.imul32p2(value1, value2),
                        16 => self.flags.imul16p2(value1, value2),
                        8 => self.flags.imul8p2(value1, value2),
                        _ => unimplemented!("wrong size"),
                    };

                    if self.cfg.test_mode {
                        if result != inline::imul2p(value1, value2, sz) {
                            panic!(
                                "imul{}p3 gives 0x{:x} and should be 0x{:x}",
                                sz,
                                result,
                                inline::imul2p(value1, value2, sz)
                            );
                        }
                    }

                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                }
            }

            Mnemonic::Bt => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let mut bit = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 1);
                if sz > 8 {
                    bit = bit % sz as u64;
                }

                if bit < 64 {
                    self.flags.f_cf = get_bit!(value, bit) == 1;
                }
            }

            Mnemonic::Btc => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let mut bitpos = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);
                bitpos = bitpos % sz as u64;

                let cf = get_bit!(value0, bitpos);
                self.flags.f_cf = cf == 1;

                let mut result = value0;
                set_bit!(result, bitpos, cf ^ 1);

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Bts => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let mut bit = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);
                bit = bit % sz as u64;

                let cf = get_bit!(value, bit);
                self.flags.f_cf = cf == 1;

                let mut result = value;
                set_bit!(result, bit, 1);

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Btr => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let mut bit = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);
                bit = bit % sz as u64;

                let cf = get_bit!(value, bit);
                self.flags.f_cf = cf == 1;

                let mut result = value;
                set_bit!(result, bit, 0);

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Bsf => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);

                if value1 == 0 {
                    self.flags.f_zf = true;

                    if self.cfg.verbose >= 1 {
                        println!("/!\\ undefined behavior on BSF with src == 0");
                    }
                } else {
                    self.flags.f_zf = false;

                    if !self.set_operand_value(&ins, 0, value1.trailing_zeros() as u64) {
                        return false;
                    }
                }

                // cf flag undefined behavior apple mac x86_64 problem
                if self.regs.rip == 0x144ed424a {
                    if self.cfg.verbose >= 1 {
                        println!("/!\\ f_cf undefined behaviour");
                    }
                    self.flags.f_cf = false;
                }

                /*
                if src == 0 {
                    self.flags.f_zf = true;
                    if self.cfg.verbose >= 1 {
                        println!("/!\\ bsf src == 0 is undefined behavior");
                    }
                } else {
                    let sz = self.get_operand_sz(&ins, 0);
                    let mut bitpos: u8 = 0;
                    let mut dest: u64 = 0;

                    while bitpos < sz && get_bit!(src, bitpos) == 0 {
                        dest += 1;
                        bitpos += 1;
                    }

                    if dest == 0 {
                        self.flags.f_zf = true;
                    } else {
                        self.flags.f_zf = false;
                    }

                    if !self.set_operand_value(&ins, 0, dest) {
                        return false;
                    }
                }*/
            }

            Mnemonic::Bsr => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);

                let (result, new_flags) = inline::bsr(value0, value1, sz, self.flags.dump());

                self.flags.load(new_flags);

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }

                /*
                if value1 == 0 {
                    self.flags.f_zf = true;
                    if self.cfg.verbose >= 1 {
                        println!("/!\\ bsr src == 0 is undefined behavior");
                    }
                } else {
                    let sz = self.get_operand_sz(&ins, 0);
                    let mut dest: u64 = sz as u64 -1;

                    while dest > 0 && get_bit!(value1, dest) == 0 {
                        dest -= 1;
                    }

                    if dest == 0 {
                        self.flags.f_zf = true;
                    } else {
                        self.flags.f_zf = false;
                    }

                    if !self.set_operand_value(&ins, 0, dest) {
                        return false;
                    }
                }*/
            }

            Mnemonic::Bswap => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 1);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1;
                let sz = self.get_operand_sz(&ins, 0);

                if sz == 32 {
                    value1 = (value0 & 0x00000000_000000ff) << 24
                        | (value0 & 0x00000000_0000ff00) << 8
                        | (value0 & 0x00000000_00ff0000) >> 8
                        | (value0 & 0x00000000_ff000000) >> 24
                        | (value0 & 0xffffffff_00000000);
                } else if sz == 64 {
                    value1 = (value0 & 0xff000000_00000000) >> 56
                        | (value0 & 0x00ff0000_00000000) >> 40
                        | (value0 & 0x0000ff00_00000000) >> 24
                        | (value0 & 0x000000ff_00000000) >> 8
                        | (value0 & 0x00000000_ff000000) << 8
                        | (value0 & 0x00000000_00ff0000) << 24
                        | (value0 & 0x00000000_0000ff00) << 40
                        | (value0 & 0x00000000_000000ff) << 56;
                } else if sz == 16 {
                    value1 = 0;
                    if self.cfg.verbose >= 1 {
                        println!("/!\\ bswap of 16bits has undefined behaviours");
                    }
                } else {
                    unimplemented!("bswap <16bits makes no sense, isn't it?");
                }

                if self.cfg.test_mode {
                    if value1 != inline::bswap(value0, sz) {
                        panic!(
                            "bswap test failed, 0x{:x} should be 0x{:x}",
                            value1,
                            inline::bswap(value0, sz)
                        );
                    }
                }

                /*
                for i in 0..sz {
                    let bit = get_bit!(value0, i);
                    set_bit!(value1, sz-i-1, bit);
                }*/

                if !self.set_operand_value(&ins, 0, value1) {
                    return false;
                }
            }

            Mnemonic::Xadd => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                if !self.set_operand_value(&ins, 1, value0) {
                    return false;
                }

                let res: u64 = match self.get_operand_sz(&ins, 1) {
                    64 => self.flags.add64(value0, value1),
                    32 => self.flags.add32(value0, value1),
                    16 => self.flags.add16(value0, value1),
                    8 => self.flags.add8(value0, value1),
                    _ => unreachable!("weird size"),
                };

                if !self.set_operand_value(&ins, 0, res) {
                    return false;
                }
            }

            Mnemonic::Ucomiss => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                assert!(ins.op_count() == 2);

                let val1 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let val2 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let low_val1 = (val1 & 0xFFFFFFFF) as u32;
                let low_val2 = (val2 & 0xFFFFFFFF) as u32;

                let f1 = f32::from_bits(low_val1);
                let f2 = f32::from_bits(low_val2);

                self.flags.f_zf = false;
                self.flags.f_pf = false;
                self.flags.f_cf = false;

                if f1.is_nan() || f2.is_nan() {
                    self.flags.f_pf = true;
                } else if f1 == f2 {
                    self.flags.f_zf = true;
                } else if f1 < f2 {
                    self.flags.f_cf = true;
                }
            }

            Mnemonic::Ucomisd => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                assert!(ins.op_count() == 2);

                let value1 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value2 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let low_val1 = (value1 & 0xFFFFFFFFFFFFFFFF) as u64;
                let low_val2 = (value2 & 0xFFFFFFFFFFFFFFFF) as u64;

                let f1 = f64::from_bits(low_val1);
                let f2 = f64::from_bits(low_val2);

                self.flags.f_zf = false;
                self.flags.f_pf = false;
                self.flags.f_cf = false;

                if f1.is_nan() || f2.is_nan() {
                    self.flags.f_pf = true;
                } else if f1 == f2 {
                    self.flags.f_zf = true;
                } else if f1 < f2 {
                    self.flags.f_cf = true;
                }
            }

            Mnemonic::Movss => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                if ins.op_count() > 2 {
                    unimplemented!("Movss with 3 operands is not implemented yet");
                }

                assert!(ins.op_count() == 2);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz1 == 128 {
                    let val = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let vf32: f32 = f32::from_bits((val & 0xFFFFFFFF) as u32);
                    let result: u32 = vf32.to_bits();

                    if !self.set_operand_value(&ins, 0, result as u64) {
                        return false;
                    }
                } else if sz0 == 128 && sz1 < 128 {
                    let val = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let value1_f32: f32 = f32::from_bits(val as u32);
                    let result: u32 = value1_f32.to_bits();
                    let xmm_value: u128 = result as u128;

                    self.set_operand_xmm_value_128(&ins, 0, xmm_value);
                } else {
                    unimplemented!("Movss unimplemented operation");
                }
            }

            Mnemonic::Movsxd => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                assert!(ins.op_count() == 2);

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let result: u64 = value1 as u32 as i32 as i64 as u64;

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Movsx => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                assert!(ins.op_count() == 2);

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                assert!(
                    (sz0 == 16 && sz1 == 8)
                        || (sz0 == 32 && sz1 == 8)
                        || (sz0 == 32 && sz1 == 16)
                        || (sz0 == 64 && sz1 == 32)
                        || (sz0 == 64 && sz1 == 16)
                        || (sz0 == 64 && sz1 == 8)
                );

                let mut result: u64 = 0;

                if sz0 == 16 {
                    assert!(sz1 == 8);
                    result = value1 as u8 as i8 as i16 as u16 as u64;
                } else if sz0 == 32 {
                    if sz1 == 8 {
                        result = value1 as u8 as i8 as i64 as u64;
                    } else if sz1 == 16 {
                        result = value1 as u16 as i16 as i32 as u32 as u64;
                    }
                } else if sz0 == 64 {
                    if sz1 == 8 {
                        result = value1 as u8 as i8 as i64 as u64;
                    } else if sz1 == 16 {
                        result = value1 as u16 as i16 as i64 as u64;
                    } else if sz1 == 32 {
                        result = value1 as u32 as i32 as i64 as u64;
                    }
                }

                if self.cfg.test_mode {
                    if result != inline::movsx(value1, sz0, sz1) {
                        panic!(
                            "MOVSX sz:{}->{}  0x{:x} should be 0x{:x}",
                            sz0,
                            sz1,
                            result,
                            inline::movsx(value1, sz0, sz1)
                        );
                    }
                }

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Movzx => {
                self.show_instruction(&self.colors.light_cyan, &ins);
                assert!(ins.op_count() == 2);

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                assert!(
                    (sz0 == 16 && sz1 == 8)
                        || (sz0 == 32 && sz1 == 8)
                        || (sz0 == 32 && sz1 == 16)
                        || (sz0 == 64 && sz1 == 32)
                        || (sz0 == 64 && sz1 == 16)
                        || (sz0 == 64 && sz1 == 8)
                );

                let result: u64;

                result = value1;

                //println!("0x{:x}: MOVZX 0x{:x}", ins.ip32(), result);

                /*
                if self.cfg.test_mode {
                    if result != inline::movzx(value1) {
                        panic!("MOVZX sz:{}->{} 0x{:x} should be 0x{:x}",
                               sz1, sz0, result, inline::movzx(value1));
                    }
                }*/

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Movsb => {
                if self.cfg.is_64bits {
                    if ins.has_rep_prefix() {
                        let mut first_iteration = true;
                        loop {
                            if first_iteration || self.cfg.verbose >= 3 {
                                self.show_instruction(&self.colors.light_cyan, &ins);
                            }
                            if !first_iteration {
                                self.pos += 1;
                            }

                            let val = match self.maps.read_byte(self.regs.rsi) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read memory on rsi");
                                    return false;
                                }
                            };
                            if !self.maps.write_byte(self.regs.rdi, val) {
                                println!("cannot write memoryh on rdi");
                                return false;
                            }

                            if !self.flags.f_df {
                                self.regs.rsi += 1;
                                self.regs.rdi += 1;
                            } else {
                                self.regs.rsi -= 1;
                                self.regs.rdi -= 1;
                            }

                            self.regs.rcx -= 1;
                            if self.regs.rcx == 0 {
                                return true;
                            }
                            first_iteration = false;
                            if rep_step {
                                self.force_reload = true;
                                break;
                            }
                        }
                    } else {
                        self.show_instruction(&self.colors.light_cyan, &ins);

                        let val = self
                            .maps
                            .read_byte(self.regs.rsi)
                            .expect("cannot read memory");
                        self.maps.write_byte(self.regs.rdi, val);
                        if !self.flags.f_df {
                            self.regs.rsi += 1;
                            self.regs.rdi += 1;
                        } else {
                            self.regs.rsi -= 1;
                            self.regs.rdi -= 1;
                        }
                    }
                } else {
                    // 32bits

                    if ins.has_rep_prefix() {
                        let mut first_iteration = true;
                        loop {
                            if first_iteration || self.cfg.verbose >= 3 {
                                self.show_instruction(&self.colors.light_cyan, &ins);
                            }
                            if !first_iteration {
                                self.pos += 1;
                            }

                            let val = match self.maps.read_byte(self.regs.get_esi()) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read memory on esi");
                                    return false;
                                }
                            };
                            if !self.maps.write_byte(self.regs.get_edi(), val) {
                                println!("cannot write memory on edi");
                                return false;
                            }

                            if !self.flags.f_df {
                                self.regs.set_esi(self.regs.get_esi() + 1);
                                self.regs.set_edi(self.regs.get_edi() + 1);
                            } else {
                                self.regs.set_esi(self.regs.get_esi() - 1);
                                self.regs.set_edi(self.regs.get_edi() - 1);
                            }

                            self.regs.set_ecx(self.regs.get_ecx() - 1);
                            if self.regs.get_ecx() == 0 {
                                return true;
                            }
                            first_iteration = false;
                            if rep_step {
                                self.force_reload = true;
                                break;
                            }
                        }
                    } else {
                        self.show_instruction(&self.colors.light_cyan, &ins);

                        let val = match self.maps.read_byte(self.regs.get_esi()) {
                            Some(v) => v,
                            None => return false,
                        };

                        self.maps.write_byte(self.regs.get_edi(), val);
                        if !self.flags.f_df {
                            self.regs.set_esi(self.regs.get_esi() + 1);
                            self.regs.set_edi(self.regs.get_edi() + 1);
                        } else {
                            self.regs.set_esi(self.regs.get_esi() - 1);
                            self.regs.set_edi(self.regs.get_edi() - 1);
                        }
                    }
                }
            }

            Mnemonic::Movsw => {
                if self.cfg.is_64bits {
                    if ins.has_rep_prefix() {
                        let mut first_iteration = true;
                        loop {
                            if first_iteration || self.cfg.verbose >= 3 {
                                self.show_instruction(&self.colors.light_cyan, &ins);
                            }
                            if !first_iteration {
                                self.pos += 1;
                            }

                            let val = self
                                .maps
                                .read_word(self.regs.rsi)
                                .expect("cannot read memory");
                            self.maps.write_word(self.regs.rdi, val);

                            if !self.flags.f_df {
                                self.regs.rsi += 2;
                                self.regs.rdi += 2;
                            } else {
                                self.regs.rsi -= 2;
                                self.regs.rdi -= 2;
                            }

                            self.regs.rcx -= 1;
                            if self.regs.rcx == 0 {
                                return true;
                            }
                            first_iteration = false;
                            if rep_step {
                                self.force_reload = true;
                                break;
                            }
                        }
                    } else {
                        self.show_instruction(&self.colors.light_cyan, &ins);
                        let val = self
                            .maps
                            .read_word(self.regs.rsi)
                            .expect("cannot read memory");
                        self.maps.write_word(self.regs.rdi, val);
                        if !self.flags.f_df {
                            self.regs.rsi += 2;
                            self.regs.rdi += 2;
                        } else {
                            self.regs.rsi -= 2;
                            self.regs.rdi -= 2;
                        }
                    }
                } else {
                    // 32bits

                    if ins.has_rep_prefix() {
                        let mut first_iteration = true;
                        loop {
                            if first_iteration || self.cfg.verbose >= 3 {
                                self.show_instruction(&self.colors.light_cyan, &ins);
                            }
                            if !first_iteration {
                                self.pos += 1;
                            }

                            let val = self
                                .maps
                                .read_word(self.regs.get_esi())
                                .expect("cannot read memory");
                            self.maps.write_word(self.regs.get_edi(), val);

                            if !self.flags.f_df {
                                self.regs.set_esi(self.regs.get_esi() + 2);
                                self.regs.set_edi(self.regs.get_edi() + 2);
                            } else {
                                self.regs.set_esi(self.regs.get_esi() - 2);
                                self.regs.set_edi(self.regs.get_edi() - 2);
                            }

                            self.regs.set_ecx(self.regs.get_ecx() - 1);
                            if self.regs.get_ecx() == 0 {
                                return true;
                            }
                            first_iteration = false;
                            if rep_step {
                                self.force_reload = true;
                                break;
                            }
                        }
                    } else {
                        self.show_instruction(&self.colors.light_cyan, &ins);
                        let val = match self.maps.read_word(self.regs.get_esi()) {
                            Some(v) => v,
                            None => {
                                println!("cannot read memory on esi");
                                return false;
                            }
                        };
                        self.maps.write_word(self.regs.get_edi(), val);
                        if !self.flags.f_df {
                            self.regs.set_esi(self.regs.get_esi() + 2);
                            self.regs.set_edi(self.regs.get_edi() + 2);
                        } else {
                            self.regs.set_esi(self.regs.get_esi() - 2);
                            self.regs.set_edi(self.regs.get_edi() - 2);
                        }
                    }
                }
            }

            Mnemonic::Movsq => {
                if ins.has_rep_prefix() {
                    let mut first_iteration = true;
                    loop {
                        if first_iteration || self.cfg.verbose >= 3 {
                            self.show_instruction(&self.colors.light_cyan, &ins);
                        }
                        if self.regs.rcx == 0 {
                            return true;
                        }
                        if !first_iteration {
                            self.pos += 1;
                        }

                        let val = self
                            .maps
                            .read_qword(self.regs.rsi)
                            .expect("cannot read memory");
                        self.maps.write_qword(self.regs.rdi, val);

                        if !self.flags.f_df {
                            self.regs.rsi += 8;
                            self.regs.rdi += 8;
                        } else {
                            self.regs.rsi -= 8;
                            self.regs.rdi -= 8;
                        }

                        self.regs.rcx -= 1;
                        if self.regs.rcx == 0 {
                            return true;
                        }
                        first_iteration = false;
                        if rep_step {
                            self.force_reload = true;
                            break;
                        }
                    }
                } else {
                    self.show_instruction(&self.colors.light_cyan, &ins);
                    let val = self
                        .maps
                        .read_qword(self.regs.rsi)
                        .expect("cannot read memory");

                    self.maps.write_qword(self.regs.rdi, val);

                    if !self.flags.f_df {
                        self.regs.rsi += 8;
                        self.regs.rdi += 8;
                    } else {
                        self.regs.rsi -= 8;
                        self.regs.rdi -= 8;
                    }
                }
            }

            Mnemonic::Movsd => {
                if ins.op_count() == 2
                    && (self.get_operand_sz(&ins, 0) == 128 || self.get_operand_sz(&ins, 1) == 128)
                {
                    self.show_instruction(&self.colors.light_cyan, &ins);
                    let src = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v & 0xffffffff_ffffffff,
                        None => return false,
                    };

                    let mut dst = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    dst = (dst & 0xffffffff_ffffffff_00000000_00000000) | src;

                    self.set_operand_xmm_value_128(&ins, 0, dst);
                } else {
                    if self.cfg.is_64bits {
                        if ins.has_rep_prefix() {
                            let mut first_iteration = true;
                            loop {
                                if first_iteration || self.cfg.verbose >= 3 {
                                    self.show_instruction(&self.colors.light_cyan, &ins);
                                }
                                if self.regs.rcx == 0 {
                                    return true;
                                }
                                if !first_iteration {
                                    self.pos += 1;
                                }

                                let val = self
                                    .maps
                                    .read_dword(self.regs.rsi)
                                    .expect("cannot read memory");

                                self.maps.write_dword(self.regs.rdi, val);

                                if !self.flags.f_df {
                                    self.regs.rsi += 4;
                                    self.regs.rdi += 4;
                                } else {
                                    self.regs.rsi -= 4;
                                    self.regs.rdi -= 4;
                                }

                                self.regs.rcx -= 1;
                                if self.regs.rcx == 0 {
                                    return true;
                                }
                                first_iteration = false;
                                if rep_step {
                                    self.force_reload = true;
                                    break;
                                }
                            }
                        } else {
                            self.show_instruction(&self.colors.light_cyan, &ins);
                            let val = self
                                .maps
                                .read_dword(self.regs.rsi)
                                .expect("cannot read memory");
                            self.maps.write_dword(self.regs.rdi, val);
                            if !self.flags.f_df {
                                self.regs.rsi += 4;
                                self.regs.rdi += 4;
                            } else {
                                self.regs.rsi -= 4;
                                self.regs.rdi -= 4;
                            }
                        }
                    } else {
                        // 32bits

                        if ins.has_rep_prefix() {
                            let mut first_iteration = true;
                            loop {
                                if first_iteration || self.cfg.verbose >= 3 {
                                    self.show_instruction(&self.colors.light_cyan, &ins);
                                }
                                if self.regs.get_ecx() == 0 {
                                    return true;
                                }
                                if !first_iteration {
                                    self.pos += 1;
                                }

                                let val = match self.maps.read_dword(self.regs.get_esi()) {
                                    Some(v) => v,
                                    None => {
                                        println!("cannot read memory at esi");
                                        return false;
                                    }
                                };
                                self.maps.write_dword(self.regs.get_edi(), val);

                                if !self.flags.f_df {
                                    self.regs.set_esi(self.regs.get_esi() + 4);
                                    self.regs.set_edi(self.regs.get_edi() + 4);
                                } else {
                                    self.regs.set_esi(self.regs.get_esi() - 4);
                                    self.regs.set_edi(self.regs.get_edi() - 4);
                                }

                                self.regs.set_ecx(self.regs.get_ecx() - 1);
                                if self.regs.get_ecx() == 0 {
                                    return true;
                                }
                                first_iteration = false;
                                if rep_step {
                                    self.force_reload = true;
                                    break;
                                }
                            }
                        } else {
                            self.show_instruction(&self.colors.light_cyan, &ins);
                            let val = match self.maps.read_dword(self.regs.get_esi()) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read memory");
                                    return false;
                                }
                            };
                            self.maps.write_dword(self.regs.get_edi(), val);
                            if !self.flags.f_df {
                                self.regs.set_esi(self.regs.get_esi() + 4);
                                self.regs.set_edi(self.regs.get_edi() + 4);
                            } else {
                                self.regs.set_esi(self.regs.get_esi() - 4);
                                self.regs.set_edi(self.regs.get_edi() - 4);
                            }
                        }
                    }
                }
            }

            Mnemonic::Cmova => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_cf && !self.flags.f_zf {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovae => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_cf {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovb => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_cf {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovbe => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_cf || self.flags.f_zf {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmove => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_zf {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovg => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_zf && self.flags.f_sf == self.flags.f_of {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovge => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_sf == self.flags.f_of {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovl => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_sf != self.flags.f_of {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovle => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_zf || self.flags.f_sf != self.flags.f_of {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovno => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_of {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovne => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_zf {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovp => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_pf {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            // https://hjlebbink.github.io/x86doc/html/CMOVcc.html
            Mnemonic::Cmovnp => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_pf {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovs => {
                self.show_instruction(&self.colors.orange, &ins);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                if self.flags.f_sf {
                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                } else {
                    // clear upper bits of register?
                    if !self.set_operand_value(&ins, 0, value0) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovns => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_sf {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Cmovo => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_of {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                }
            }

            Mnemonic::Seta => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_cf && !self.flags.f_zf {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setae => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_cf {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setb => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_cf {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setbe => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_cf || self.flags.f_zf {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Sete => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_zf {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setg => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_zf && self.flags.f_sf == self.flags.f_of {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setge => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_sf == self.flags.f_of {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setl => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_sf != self.flags.f_of {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setle => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_zf || self.flags.f_sf != self.flags.f_of {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setne => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_zf {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setno => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_of {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setnp => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_pf {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setns => {
                self.show_instruction(&self.colors.orange, &ins);

                if !self.flags.f_sf {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Seto => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_of {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Setp => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_pf {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Sets => {
                self.show_instruction(&self.colors.orange, &ins);

                if self.flags.f_sf {
                    if !self.set_operand_value(&ins, 0, 1) {
                        return false;
                    }
                } else {
                    if !self.set_operand_value(&ins, 0, 0) {
                        return false;
                    }
                }
            }

            Mnemonic::Stosb => {
                if ins.has_rep_prefix() {
                    let mut first_iteration = true;
                    loop {
                        if first_iteration || self.cfg.verbose >= 3 {
                            self.show_instruction(&self.colors.light_cyan, &ins);
                        }
                        if !first_iteration {
                            self.pos += 1;
                        }

                        if self.regs.rcx == 0 {
                            return true;
                        }

                        if self.cfg.is_64bits {
                            if !self
                                .maps
                                .write_byte(self.regs.rdi, self.regs.get_al() as u8)
                            {
                                return false;
                            }
                            if self.flags.f_df {
                                self.regs.rdi -= 1;
                            } else {
                                self.regs.rdi += 1;
                            }
                        } else {
                            // 32bits
                            if !self
                                .maps
                                .write_byte(self.regs.get_edi(), self.regs.get_al() as u8)
                            {
                                return false;
                            }
                            if self.flags.f_df {
                                self.regs.set_edi(self.regs.get_edi() - 1);
                            } else {
                                self.regs.set_edi(self.regs.get_edi() + 1);
                            }
                        }

                        self.regs.rcx -= 1;
                        first_iteration = false;
                        if rep_step {
                            self.force_reload = true;
                            break;
                        }
                    }
                } else {
                    if self.cfg.is_64bits {
                        self.maps
                            .write_byte(self.regs.rdi, self.regs.get_al() as u8);
                        if self.flags.f_df {
                            self.regs.rdi -= 1;
                        } else {
                            self.regs.rdi += 1;
                        }
                    } else {
                        // 32bits
                        self.maps
                            .write_byte(self.regs.get_edi(), self.regs.get_al() as u8);
                        if self.flags.f_df {
                            self.regs.set_edi(self.regs.get_edi() - 1);
                        } else {
                            self.regs.set_edi(self.regs.get_edi() + 1);
                        }
                    }
                }
            }

            Mnemonic::Stosw => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                if self.cfg.is_64bits {
                    self.maps
                        .write_word(self.regs.rdi, self.regs.get_ax() as u16);

                    if self.flags.f_df {
                        self.regs.rdi -= 2;
                    } else {
                        self.regs.rdi += 2;
                    }
                } else {
                    // 32bits
                    self.maps
                        .write_word(self.regs.get_edi(), self.regs.get_ax() as u16);

                    if self.flags.f_df {
                        self.regs.set_edi(self.regs.get_edi() - 2);
                    } else {
                        self.regs.set_edi(self.regs.get_edi() + 2);
                    }
                }
            }

            Mnemonic::Stosd => {
                if ins.has_rep_prefix() {
                    let mut first_iteration = true;
                    loop {
                        if first_iteration || self.cfg.verbose >= 3 {
                            self.show_instruction(&self.colors.light_cyan, &ins);
                        }
                        if !first_iteration {
                            self.pos += 1;
                        }

                        if self.regs.rcx == 0 {
                            return true;
                        }

                        if self.cfg.is_64bits {
                            if !self
                                .maps
                                .write_dword(self.regs.rdi, self.regs.get_eax() as u32)
                            {
                                return false;
                            }
                            if self.flags.f_df {
                                self.regs.rdi -= 4;
                            } else {
                                self.regs.rdi += 4;
                            }
                        } else {
                            // 32bits
                            if !self
                                .maps
                                .write_dword(self.regs.get_edi(), self.regs.get_eax() as u32)
                            {
                                return false;
                            }

                            if self.flags.f_df {
                                self.regs.set_edi(self.regs.get_edi() - 4);
                            } else {
                                self.regs.set_edi(self.regs.get_edi() + 4);
                            }
                        }

                        self.regs.rcx -= 1;
                        first_iteration = false;
                        if rep_step {
                            self.force_reload = true;
                            break;
                        }
                    }
                } else {
                    self.show_instruction(&self.colors.light_cyan, &ins);
                    if self.cfg.is_64bits {
                        self.maps
                            .write_dword(self.regs.rdi, self.regs.get_eax() as u32);

                        if self.flags.f_df {
                            self.regs.rdi -= 4;
                        } else {
                            self.regs.rdi += 4;
                        }
                    } else {
                        // 32bits
                        self.maps
                            .write_dword(self.regs.get_edi(), self.regs.get_eax() as u32);

                        if self.flags.f_df {
                            self.regs.set_edi(self.regs.get_edi() - 4);
                        } else {
                            self.regs.set_edi(self.regs.get_edi() + 4);
                        }
                    }
                }
            }

            Mnemonic::Stosq => {
                if ins.has_rep_prefix() {
                    let mut first_iteration = true;
                    loop {
                        if first_iteration || self.cfg.verbose >= 3 {
                            self.show_instruction(&self.colors.light_cyan, &ins);
                        }
                        if !first_iteration {
                            // this is for the diff2.py diffing with gdb that
                            // unrolls the reps
                            if self.cfg.verbose > 2 {
                                println!("\t{} rip: 0x{:x}", self.pos, self.regs.rip);
                            }
                        }

                        self.maps.write_qword(self.regs.rdi, self.regs.rax);

                        if self.flags.f_df {
                            self.regs.rdi -= 8;
                        } else {
                            self.regs.rdi += 8;
                        }

                        self.regs.rcx -= 1;
                        if self.regs.rcx == 0 {
                            return true;
                        }

                        first_iteration = false;
                        if rep_step {
                            self.force_reload = true;
                            break;
                        }
                        self.pos += 1;
                    }
                } else {
                    self.show_instruction(&self.colors.light_cyan, &ins);

                    self.maps.write_qword(self.regs.rdi, self.regs.rax);

                    if self.flags.f_df {
                        self.regs.rdi -= 8;
                    } else {
                        self.regs.rdi += 8;
                    }
                }
            }

            Mnemonic::Scasb => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                let value0: u64 = match self.maps.read_byte(self.regs.rdi) {
                    Some(value) => value.into(),
                    None => {
                        println!("/!\\ error reading byte on rdi 0x{:x}", self.regs.rdi);
                        return false;
                    }
                };

                self.flags.sub8(self.regs.get_al(), value0);

                if self.cfg.is_64bits {
                    if self.flags.f_df {
                        self.regs.rdi -= 1;
                    } else {
                        self.regs.rdi += 1;
                    }
                } else {
                    // 32bits
                    if self.flags.f_df {
                        self.regs.set_edi(self.regs.get_edi() - 1);
                    } else {
                        self.regs.set_edi(self.regs.get_edi() + 1);
                    }
                }
            }

            Mnemonic::Scasw => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                self.flags.sub16(self.regs.get_ax(), value0);

                if self.cfg.is_64bits {
                    if self.flags.f_df {
                        self.regs.rdi -= 2;
                    } else {
                        self.regs.rdi += 2;
                    }
                } else {
                    // 32bits
                    if self.flags.f_df {
                        self.regs.set_edi(self.regs.get_edi() - 2);
                    } else {
                        self.regs.set_edi(self.regs.get_edi() + 2);
                    }
                }
            }

            Mnemonic::Scasd => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                self.flags.sub32(self.regs.get_eax(), value0);

                if self.cfg.is_64bits {
                    if self.flags.f_df {
                        self.regs.rdi -= 4;
                    } else {
                        self.regs.rdi += 4;
                    }
                } else {
                    // 32bits
                    if self.flags.f_df {
                        self.regs.set_edi(self.regs.get_edi() - 4);
                    } else {
                        self.regs.set_edi(self.regs.get_edi() + 4);
                    }
                }
            }

            Mnemonic::Scasq => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                self.flags.sub64(self.regs.rax, value0);

                if self.flags.f_df {
                    self.regs.rdi -= 8;
                } else {
                    self.regs.rdi += 8;
                }
            }

            Mnemonic::Test => {
                self.show_instruction(&self.colors.orange, &ins);

                assert!(ins.op_count() == 2);

                if self.break_on_next_cmp {
                    self.spawn_console();
                    self.break_on_next_cmp = false;
                }

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);

                self.flags.test(value0, value1, sz);
            }

            Mnemonic::Cmpxchg => {
                self.show_instruction(&self.colors.orange, &ins);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                if self.cfg.is_64bits {
                    if value0 == self.regs.rax {
                        self.flags.f_zf = true;
                        if !self.set_operand_value(&ins, 0, value1) {
                            return false;
                        }
                    } else {
                        self.flags.f_zf = false;
                        self.regs.rax = value1;
                    }
                } else {
                    // 32bits
                    if value0 == self.regs.get_eax() {
                        self.flags.f_zf = true;
                        if !self.set_operand_value(&ins, 0, value1) {
                            return false;
                        }
                    } else {
                        self.flags.f_zf = false;
                        self.regs.set_eax(value1);
                    }
                }
            }

            Mnemonic::Cmpxchg8b => {
                self.show_instruction(&self.colors.orange, &ins);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                if value0 as u8 == (self.regs.get_al() as u8) {
                    self.flags.f_zf = true;
                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                } else {
                    self.flags.f_zf = false;
                    self.regs.set_al(value1 & 0xff);
                }
            }

            Mnemonic::Cmpxchg16b => {
                self.show_instruction(&self.colors.orange, &ins);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                if value0 as u16 == (self.regs.get_ax() as u16) {
                    self.flags.f_zf = true;
                    if !self.set_operand_value(&ins, 0, value1) {
                        return false;
                    }
                } else {
                    self.flags.f_zf = false;
                    self.regs.set_ax(value1 & 0xffff);
                }
            }

            Mnemonic::Cmp => {
                self.show_instruction(&self.colors.orange, &ins);

                assert!(ins.op_count() == 2);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                if self.cfg.verbose >= 2 {
                    if value0 > value1 {
                        println!("\tcmp: 0x{:x} > 0x{:x}", value0, value1);
                    } else if value0 < value1 {
                        println!("\tcmp: 0x{:x} < 0x{:x}", value0, value1);
                    } else {
                        println!("\tcmp: 0x{:x} == 0x{:x}", value0, value1);
                    }
                }

                if self.break_on_next_cmp {
                    self.spawn_console();
                    self.break_on_next_cmp = false;

                    let value0 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.verbose >= 2 {
                        if value0 > value1 {
                            println!("\tcmp: 0x{:x} > 0x{:x}", value0, value1);
                        } else if value0 < value1 {
                            println!("\tcmp: 0x{:x} < 0x{:x}", value0, value1);
                        } else {
                            println!("\tcmp: 0x{:x} == 0x{:x}", value0, value1);
                        }
                    }
                }

                match self.get_operand_sz(&ins, 0) {
                    64 => {
                        self.flags.sub64(value0, value1);
                    }
                    32 => {
                        self.flags.sub32(value0, value1);
                    }
                    16 => {
                        self.flags.sub16(value0, value1);
                    }
                    8 => {
                        self.flags.sub8(value0, value1);
                    }
                    _ => {
                        panic!("wrong size {}", self.get_operand_sz(&ins, 0));
                    }
                }
            }

            Mnemonic::Cmpsq => {
                let mut value0: u64;
                let mut value1: u64;

                if ins.has_rep_prefix() {
                    let mut first_iteration = true;
                    loop {
                        if first_iteration || self.cfg.verbose >= 3 {
                            self.show_instruction(&self.colors.light_cyan, &ins);
                        }
                        if !first_iteration {
                            self.pos += 1;
                        }

                        if self.cfg.is_64bits {
                            value0 = match self.maps.read_qword(self.regs.rsi) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read rsi");
                                    return false;
                                }
                            };
                            value1 = match self.maps.read_qword(self.regs.rdi) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read rdi");
                                    return false;
                                }
                            };

                            if self.flags.f_df {
                                self.regs.rsi -= 8;
                                self.regs.rdi -= 8;
                            } else {
                                self.regs.rsi += 8;
                                self.regs.rdi += 8;
                            }
                        } else {
                            // 32bits
                            value0 = match self.maps.read_qword(self.regs.get_esi()) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read esi");
                                    return false;
                                }
                            };
                            value1 = match self.maps.read_qword(self.regs.get_edi()) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read edi");
                                    return false;
                                }
                            };

                            if self.flags.f_df {
                                self.regs.set_esi(self.regs.get_esi() - 8);
                                self.regs.set_edi(self.regs.get_edi() - 8);
                            } else {
                                self.regs.set_esi(self.regs.get_esi() + 8);
                                self.regs.set_edi(self.regs.get_edi() + 8);
                            }
                        }

                        self.flags.sub64(value0, value1);

                        if value0 > value1 {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} > 0x{:x}", value0, value1);
                            }
                            return false;
                        } else if value0 < value1 {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} < 0x{:x}", value0, value1);
                            }
                            return false;
                        } else {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} == 0x{:x}", value0, value1);
                            }
                        }

                        self.regs.rcx -= 1;
                        if self.regs.rcx == 0 {
                            return true;
                        }

                        first_iteration = false;
                        if rep_step {
                            self.force_reload = true;
                            break;
                        }
                    }
                } else {
                    // not rep

                    self.show_instruction(&self.colors.orange, &ins);

                    if self.cfg.is_64bits {
                        value0 = match self.maps.read_qword(self.regs.rsi) {
                            Some(v) => v,
                            None => {
                                println!("cannot read rsi");
                                return false;
                            }
                        };
                        value1 = match self.maps.read_qword(self.regs.rdi) {
                            Some(v) => v,
                            None => {
                                println!("cannot read rdi");
                                return false;
                            }
                        };

                        if self.flags.f_df {
                            self.regs.rsi -= 8;
                            self.regs.rdi -= 8;
                        } else {
                            self.regs.rsi += 8;
                            self.regs.rdi += 8;
                        }
                    } else {
                        // 32bits
                        value0 = match self.maps.read_qword(self.regs.get_esi()) {
                            Some(v) => v,
                            None => {
                                println!("cannot read esi");
                                return false;
                            }
                        };
                        value1 = match self.maps.read_qword(self.regs.get_edi()) {
                            Some(v) => v,
                            None => {
                                println!("cannot read edi");
                                return false;
                            }
                        };

                        if self.flags.f_df {
                            self.regs.set_esi(self.regs.get_esi() - 8);
                            self.regs.set_edi(self.regs.get_edi() - 8);
                        } else {
                            self.regs.set_esi(self.regs.get_esi() + 8);
                            self.regs.set_edi(self.regs.get_edi() + 8);
                        }
                    }

                    self.flags.sub64(value0, value1);

                    if self.cfg.verbose >= 2 {
                        if value0 > value1 {
                            println!("\tcmp: 0x{:x} > 0x{:x}", value0, value1);
                        } else if value0 < value1 {
                            println!("\tcmp: 0x{:x} < 0x{:x}", value0, value1);
                        } else {
                            println!("\tcmp: 0x{:x} == 0x{:x}", value0, value1);
                        }
                    }
                }
            }

            Mnemonic::Cmpsd => {
                let mut value0: u32;
                let mut value1: u32;

                if ins.has_rep_prefix() {
                    let mut first_iteration = true;
                    loop {
                        if first_iteration || self.cfg.verbose >= 3 {
                            self.show_instruction(&self.colors.light_cyan, &ins);
                        }
                        if !first_iteration {
                            self.pos += 1;
                        }

                        if self.cfg.is_64bits {
                            value0 = match self.maps.read_dword(self.regs.rsi) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read rsi");
                                    return false;
                                }
                            };
                            value1 = match self.maps.read_dword(self.regs.rdi) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read rdi");
                                    return false;
                                }
                            };

                            if self.flags.f_df {
                                self.regs.rsi -= 4;
                                self.regs.rdi -= 4;
                            } else {
                                self.regs.rsi += 4;
                                self.regs.rdi += 4;
                            }
                        } else {
                            // 32bits
                            value0 = match self.maps.read_dword(self.regs.get_esi()) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read esi");
                                    return false;
                                }
                            };
                            value1 = match self.maps.read_dword(self.regs.get_edi()) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read edi");
                                    return false;
                                }
                            };

                            if self.flags.f_df {
                                self.regs.set_esi(self.regs.get_esi() - 4);
                                self.regs.set_edi(self.regs.get_edi() - 4);
                            } else {
                                self.regs.set_esi(self.regs.get_esi() + 4);
                                self.regs.set_edi(self.regs.get_edi() + 4);
                            }
                        }

                        self.flags.sub32(value0 as u64, value1 as u64);

                        if value0 > value1 {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} > 0x{:x}", value0, value1);
                            }
                            return false;
                        } else if value0 < value1 {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} < 0x{:x}", value0, value1);
                            }
                            return false;
                        } else {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} == 0x{:x}", value0, value1);
                            }
                        }

                        self.regs.rcx -= 1;
                        if self.regs.rcx == 0 {
                            return true;
                        }

                        first_iteration = false;
                        if rep_step {
                            self.force_reload = true;
                            break;
                        }
                    }
                } else {
                    // no rep

                    self.show_instruction(&self.colors.light_cyan, &ins);

                    if self.cfg.is_64bits {
                        value0 = match self.maps.read_dword(self.regs.rsi) {
                            Some(v) => v,
                            None => {
                                println!("cannot read rsi");
                                return false;
                            }
                        };
                        value1 = match self.maps.read_dword(self.regs.rdi) {
                            Some(v) => v,
                            None => {
                                println!("cannot read rdi");
                                return false;
                            }
                        };

                        if self.flags.f_df {
                            self.regs.rsi -= 4;
                            self.regs.rdi -= 4;
                        } else {
                            self.regs.rsi += 4;
                            self.regs.rdi += 4;
                        }
                    } else {
                        // 32bits
                        value0 = match self.maps.read_dword(self.regs.get_esi()) {
                            Some(v) => v,
                            None => {
                                println!("cannot read esi");
                                return false;
                            }
                        };
                        value1 = match self.maps.read_dword(self.regs.get_edi()) {
                            Some(v) => v,
                            None => {
                                println!("cannot read edi");
                                return false;
                            }
                        };

                        if self.flags.f_df {
                            self.regs.set_esi(self.regs.get_esi() - 4);
                            self.regs.set_edi(self.regs.get_edi() - 4);
                        } else {
                            self.regs.set_esi(self.regs.get_esi() + 4);
                            self.regs.set_edi(self.regs.get_edi() + 4);
                        }
                    }

                    self.flags.sub32(value0 as u64, value1 as u64);

                    if self.cfg.verbose >= 2 {
                        if value0 > value1 {
                            println!("\tcmp: 0x{:x} > 0x{:x}", value0, value1);
                        } else if value0 < value1 {
                            println!("\tcmp: 0x{:x} < 0x{:x}", value0, value1);
                        } else {
                            println!("\tcmp: 0x{:x} == 0x{:x}", value0, value1);
                        }
                    }
                }
            }

            Mnemonic::Cmpsw => {
                let mut value0: u16;
                let mut value1: u16;

                if ins.has_rep_prefix() {
                    let mut first_iteration = true;
                    loop {
                        if first_iteration || self.cfg.verbose >= 3 {
                            self.show_instruction(&self.colors.light_cyan, &ins);
                        }
                        if !first_iteration {
                            self.pos += 1;
                        }

                        if self.cfg.is_64bits {
                            value0 = match self.maps.read_word(self.regs.rsi) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read rsi");
                                    return false;
                                }
                            };
                            value1 = match self.maps.read_word(self.regs.rdi) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read rdi");
                                    return false;
                                }
                            };

                            if self.flags.f_df {
                                self.regs.rsi -= 1;
                                self.regs.rdi -= 1;
                            } else {
                                self.regs.rsi += 1;
                                self.regs.rdi += 1;
                            }
                        } else {
                            // 32bits
                            value0 = match self.maps.read_word(self.regs.get_esi()) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read esi");
                                    return false;
                                }
                            };
                            value1 = match self.maps.read_word(self.regs.get_edi()) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read edi");
                                    return false;
                                }
                            };

                            if self.flags.f_df {
                                self.regs.set_esi(self.regs.get_esi() - 2);
                                self.regs.set_edi(self.regs.get_edi() - 2);
                            } else {
                                self.regs.set_esi(self.regs.get_esi() + 2);
                                self.regs.set_edi(self.regs.get_edi() + 2);
                            }
                        }

                        self.flags.sub16(value0 as u64, value1 as u64);

                        if value0 > value1 {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} > 0x{:x}", value0, value1);
                            }
                            break;
                        } else if value0 < value1 {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} < 0x{:x}", value0, value1);
                            }
                            break;
                        } else {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} == 0x{:x}", value0, value1);
                            }
                        }

                        self.regs.rcx -= 1;
                        if self.regs.rcx == 0 {
                            return true;
                        }

                        first_iteration = false;
                        if rep_step {
                            self.force_reload = true;
                            break;
                        }
                    }
                } else {
                    // no rep

                    self.show_instruction(&self.colors.light_cyan, &ins);

                    if self.cfg.is_64bits {
                        value0 = match self.maps.read_word(self.regs.rsi) {
                            Some(v) => v,
                            None => {
                                println!("cannot read rsi");
                                return false;
                            }
                        };
                        value1 = match self.maps.read_word(self.regs.rdi) {
                            Some(v) => v,
                            None => {
                                println!("cannot read rdi");
                                return false;
                            }
                        };

                        if self.flags.f_df {
                            self.regs.rsi -= 1;
                            self.regs.rdi -= 1;
                        } else {
                            self.regs.rsi += 1;
                            self.regs.rdi += 1;
                        }
                    } else {
                        // 32bits
                        value0 = match self.maps.read_word(self.regs.get_esi()) {
                            Some(v) => v,
                            None => {
                                println!("cannot read esi");
                                return false;
                            }
                        };
                        value1 = match self.maps.read_word(self.regs.get_edi()) {
                            Some(v) => v,
                            None => {
                                println!("cannot read edi");
                                return false;
                            }
                        };

                        if self.flags.f_df {
                            self.regs.set_esi(self.regs.get_esi() - 2);
                            self.regs.set_edi(self.regs.get_edi() - 2);
                        } else {
                            self.regs.set_esi(self.regs.get_esi() + 2);
                            self.regs.set_edi(self.regs.get_edi() + 2);
                        }
                    }

                    self.flags.sub16(value0 as u64, value1 as u64);

                    if self.cfg.verbose >= 2 {
                        if value0 > value1 {
                            println!("\tcmp: 0x{:x} > 0x{:x}", value0, value1);
                        } else if value0 < value1 {
                            println!("\tcmp: 0x{:x} < 0x{:x}", value0, value1);
                        } else {
                            println!("\tcmp: 0x{:x} == 0x{:x}", value0, value1);
                        }
                    }
                }
            }

            Mnemonic::Cmpsb => {
                let mut value0: u8;
                let mut value1: u8;

                if ins.has_rep_prefix() {
                    let mut first_iteration = true;
                    loop {
                        if first_iteration || self.cfg.verbose >= 3 {
                            self.show_instruction(&self.colors.light_cyan, &ins);
                        }
                        if !first_iteration {
                            self.pos += 1;
                        }

                        if self.cfg.is_64bits {
                            value0 = match self.maps.read_byte(self.regs.rsi) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read rsi");
                                    return false;
                                }
                            };
                            value1 = match self.maps.read_byte(self.regs.rdi) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read rdi");
                                    return false;
                                }
                            };

                            if self.flags.f_df {
                                self.regs.rsi -= 1;
                                self.regs.rdi -= 1;
                            } else {
                                self.regs.rsi += 1;
                                self.regs.rdi += 1;
                            }
                        } else {
                            // 32bits
                            value0 = match self.maps.read_byte(self.regs.get_esi()) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read esi");
                                    return false;
                                }
                            };
                            value1 = match self.maps.read_byte(self.regs.get_edi()) {
                                Some(v) => v,
                                None => {
                                    println!("cannot read edi");
                                    return false;
                                }
                            };

                            if self.flags.f_df {
                                self.regs.set_esi(self.regs.get_esi() - 1);
                                self.regs.set_edi(self.regs.get_edi() - 1);
                            } else {
                                self.regs.set_esi(self.regs.get_esi() + 1);
                                self.regs.set_edi(self.regs.get_edi() + 1);
                            }
                        } // end 32bits

                        self.flags.sub8(value0 as u64, value1 as u64);

                        if value0 > value1 {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} > 0x{:x}", value0, value1);
                            }
                            assert!(self.flags.f_zf == false);
                            break;
                            //return false;
                        } else if value0 < value1 {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} < 0x{:x}", value0, value1);
                            }
                            assert!(self.flags.f_zf == false);
                            break;
                            //return false;
                        } else {
                            if self.cfg.verbose >= 2 {
                                println!("\tcmp: 0x{:x} == 0x{:x}", value0, value1);
                            }
                            assert!(self.flags.f_zf == true);
                        }

                        self.regs.rcx -= 1;
                        if self.regs.rcx == 0 {
                            return true;
                        }

                        first_iteration = false;
                        if rep_step {
                            self.force_reload = true;
                            break;
                        }
                    } // end rep loop
                } else {
                    // no rep

                    self.show_instruction(&self.colors.light_cyan, &ins);

                    if self.cfg.is_64bits {
                        value0 = match self.maps.read_byte(self.regs.rsi) {
                            Some(v) => v,
                            None => {
                                println!("cannot read rsi");
                                return false;
                            }
                        };
                        value1 = match self.maps.read_byte(self.regs.rdi) {
                            Some(v) => v,
                            None => {
                                println!("cannot read rdi");
                                return false;
                            }
                        };

                        if self.flags.f_df {
                            self.regs.rsi -= 1;
                            self.regs.rdi -= 1;
                        } else {
                            self.regs.rsi += 1;
                            self.regs.rdi += 1;
                        }
                    } else {
                        // 32bits
                        value0 = match self.maps.read_byte(self.regs.get_esi()) {
                            Some(v) => v,
                            None => {
                                println!("cannot read esi");
                                return false;
                            }
                        };
                        value1 = match self.maps.read_byte(self.regs.get_edi()) {
                            Some(v) => v,
                            None => {
                                println!("cannot read edi");
                                return false;
                            }
                        };

                        if self.flags.f_df {
                            self.regs.set_esi(self.regs.get_esi() - 1);
                            self.regs.set_edi(self.regs.get_edi() - 1);
                        } else {
                            self.regs.set_esi(self.regs.get_esi() + 1);
                            self.regs.set_edi(self.regs.get_edi() + 1);
                        }
                    }

                    self.flags.sub8(value0 as u64, value1 as u64);

                    if self.cfg.verbose >= 2 {
                        if value0 > value1 {
                            println!("\tcmp: 0x{:x} > 0x{:x}", value0, value1);
                        } else if value0 < value1 {
                            println!("\tcmp: 0x{:x} < 0x{:x}", value0, value1);
                        } else {
                            println!("\tcmp: 0x{:x} == 0x{:x}", value0, value1);
                        }
                    }
                }
            }

            //branches: https://web.itu.edu.tr/kesgin/mul06/intel/instr/jxx.html
            //          https://c9x.me/x86/html/file_module_x86_id_146.html
            //          http://unixwiz.net/techtips/x86-jumps.html <---aqui

            //esquema global -> https://en.wikipedia.org/wiki/X86_instruction_listings
            // test jnle jpe jpo loopz loopnz int 0x80
            Mnemonic::Jo => {
                assert!(ins.op_count() == 1);

                if self.flags.f_of {
                    self.show_instruction_taken(&self.colors.orange, &ins);

                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jno => {
                assert!(ins.op_count() == 1);

                if !self.flags.f_of {
                    self.show_instruction_taken(&self.colors.orange, &ins);

                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Js => {
                assert!(ins.op_count() == 1);

                if self.flags.f_sf {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jns => {
                assert!(ins.op_count() == 1);

                if !self.flags.f_sf {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Je => {
                assert!(ins.op_count() == 1);

                if self.flags.f_zf {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jne => {
                assert!(ins.op_count() == 1);

                if !self.flags.f_zf {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jb => {
                assert!(ins.op_count() == 1);

                if self.flags.f_cf {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jae => {
                assert!(ins.op_count() == 1);

                if !self.flags.f_cf {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jbe => {
                assert!(ins.op_count() == 1);

                if self.flags.f_cf || self.flags.f_zf {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Ja => {
                assert!(ins.op_count() == 1);

                if !self.flags.f_cf && !self.flags.f_zf {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jl => {
                assert!(ins.op_count() == 1);

                if self.flags.f_sf != self.flags.f_of {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jge => {
                assert!(ins.op_count() == 1);

                if self.flags.f_sf == self.flags.f_of {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jle => {
                assert!(ins.op_count() == 1);

                if self.flags.f_zf || self.flags.f_sf != self.flags.f_of {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jg => {
                assert!(ins.op_count() == 1);

                if !self.flags.f_zf && self.flags.f_sf == self.flags.f_of {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jp => {
                assert!(ins.op_count() == 1);

                if self.flags.f_pf {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jnp => {
                assert!(ins.op_count() == 1);

                if !self.flags.f_pf {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jcxz => {
                assert!(ins.op_count() == 1);

                if self.regs.get_cx() == 0 {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jecxz => {
                assert!(ins.op_count() == 1);

                if self.regs.get_cx() == 0 {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Jrcxz => {
                if self.regs.rcx == 0 {
                    self.show_instruction_taken(&self.colors.orange, &ins);
                    let addr = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => return false,
                    };

                    if self.cfg.is_64bits {
                        return self.set_rip(addr, true);
                    } else {
                        return self.set_eip(addr, true);
                    }
                } else {
                    self.show_instruction_not_taken(&self.colors.orange, &ins);
                }
            }

            Mnemonic::Int3 => {
                self.show_instruction(&self.colors.red, &ins);
                println!("/!\\ int 3 sigtrap!!!!");
                self.exception();
                return true;
            }

            Mnemonic::Nop => {
                self.show_instruction(&self.colors.light_purple, &ins);
            }

            Mnemonic::Fnop => {
                self.show_instruction(&self.colors.light_purple, &ins);
            }

            Mnemonic::Mfence | Mnemonic::Lfence | Mnemonic::Sfence => {
                self.show_instruction(&self.colors.red, &ins);
            }

            Mnemonic::Cpuid => {
                self.show_instruction(&self.colors.red, &ins);

                // guloader checks bit31 which is if its hipervisor with command
                // https://c9x.me/x86/html/file_module_x86_id_45.html
                // TODO: implement 0x40000000 -> get the virtualization vendor

                if self.cfg.verbose >= 1 {
                    println!(
                        "\tcpuid input value: 0x{:x}, 0x{:x}",
                        self.regs.rax, self.regs.rcx
                    );
                }

                match self.regs.rax {
                    0x00 => {
                        self.regs.rax = 0x16;
                        self.regs.rbx = 0x756e6547;
                        self.regs.rcx = 0x6c65746e;
                        self.regs.rdx = 0x49656e69;
                    }
                    0x01 => {
                        self.regs.rax = 0x906ed; // Version Information (Type, Family, Model, and Stepping ID)
                        self.regs.rbx = 0x5100800;
                        self.regs.rcx = 0x7ffafbbf;
                        self.regs.rdx = 0xbfebfbff; // feature
                    }
                    0x02 => {
                        self.regs.rax = 0x76036301;
                        self.regs.rbx = 0xf0b5ff;
                        self.regs.rcx = 0;
                        self.regs.rdx = 0xc30000;
                    }
                    0x03 => {
                        self.regs.rax = 0;
                        self.regs.rbx = 0;
                        self.regs.rcx = 0;
                        self.regs.rdx = 0;
                    }
                    0x04 => {
                        self.regs.rax = 0;
                        self.regs.rbx = 0x1c0003f;
                        self.regs.rcx = 0x3f;
                        self.regs.rdx = 0;
                    }
                    0x05 => {
                        self.regs.rax = 0x40;
                        self.regs.rbx = 0x40;
                        self.regs.rcx = 3;
                        self.regs.rdx = 0x11142120;
                    }
                    0x06 => {
                        self.regs.rax = 0x27f7;
                        self.regs.rbx = 2;
                        self.regs.rcx = 9;
                        self.regs.rdx = 0;
                    }
                    0x0d => {
                        match self.regs.rcx {
                            1 => {
                                self.regs.rax = 0xf;
                                self.regs.rbx = 0x3c0;
                                self.regs.rcx = 0x100;
                                self.regs.rdx = 0;
                            }
                            0 => {
                                self.regs.rax = 0x1f;
                                self.regs.rbx = 0x440;
                                self.regs.rcx = 0x440;
                                self.regs.rdx = 0;
                            }
                            2 => {
                                self.regs.rax = 0x100;
                                self.regs.rbx = 0x240;
                                self.regs.rcx = 0;
                                self.regs.rdx = 0;
                            }
                            3 => {
                                self.regs.rax = 0x40;
                                self.regs.rbx = 0x3c0;
                                self.regs.rcx = 0;
                                self.regs.rdx = 0;
                            }
                            5 | 6 | 7 => {
                                self.regs.rax = 0;
                                self.regs.rbx = 0;
                                self.regs.rcx = 0;
                                self.regs.rdx = 0;
                            }
                            _ => {
                                self.regs.rax = 0x1f; //0x1f
                                self.regs.rbx = 0x440; //0x3c0; // 0x440
                                self.regs.rcx = 0x440; //0x100; // 0x440
                                self.regs.rdx = 0;
                            }
                        }
                    }
                    0x07..=0x6d => {
                        self.regs.rax = 0;
                        self.regs.rbx = 0x29c67af;
                        self.regs.rcx = 0x40000000;
                        self.regs.rdx = 0xbc000600;
                    }
                    0x6e => {
                        self.regs.rax = 0x960;
                        self.regs.rbx = 0x1388;
                        self.regs.rcx = 0x64;
                        self.regs.rdx = 0;
                    }
                    0x80000000 => {
                        self.regs.rax = 0x80000008;
                        self.regs.rbx = 0;
                        self.regs.rcx = 0;
                        self.regs.rdx = 0;
                    }
                    0x80000001 => {
                        self.regs.rax = 0;
                        self.regs.rbx = 0;
                        self.regs.rcx = 0x121;
                        self.regs.rdx = 0x2c100800;
                        self.regs.rsi = 0x80000008;
                    }
                    0x80000007 => {
                        self.regs.rax = 0;
                        self.regs.rbx = 0;
                        self.regs.rcx = 0;
                        self.regs.rdx = 0x100;
                    }
                    0x80000008 => {
                        self.regs.rax = 0x3027;
                        self.regs.rbx = 0;
                        self.regs.rcx = 0;
                        self.regs.rdx = 0; //0x100;
                    }
                    _ => {
                        println!("unimplemented cpuid call 0x{:x}", self.regs.rax);
                        return false;
                    }
                }
            }

            Mnemonic::Clc => {
                self.show_instruction(&self.colors.light_gray, &ins);
                self.flags.f_cf = false;
            }

            Mnemonic::Rdtsc => {
                self.show_instruction(&self.colors.red, &ins);

                let elapsed = self.now.elapsed();
                let cycles: u64 = elapsed.as_nanos() as u64;
                self.regs.rax = (cycles & 0xffffffff) as u64;
                self.regs.rdx = (cycles >> 32) as u64;

                if self.cfg.is_64bits {
                    /*
                    let rax:u64;
                    let rdx:u64;
                    unsafe {
                        asm!(
                            "rdtsc",
                            "mov {}, rax",
                            "mov {}, rdx",
                            out(reg) rax,
                            out(reg) rdx
                        );
                    }
                    self.regs.rax = rax;
                    self.regs.rdx = rdx;
                    */
                } else { // 32bits

                    /*
                    // TODO: actually mock a timestamp?
                    self.regs.rdx = 0x1BC2B;
                    self.regs.rax = 0xE6668424;
                    self.flags.f_pf = true;
                    self.flags.f_af = false;
                    */
                }
            }

            Mnemonic::Loop => {
                self.show_instruction(&self.colors.yellow, &ins);

                assert!(ins.op_count() == 1);

                let addr = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                if addr > 0xffffffff {
                    if self.regs.rcx == 0 {
                        self.regs.rcx = 0xffffffffffffffff;
                    } else {
                        self.regs.rcx -= 1;
                    }

                    if self.regs.rcx > 0 {
                        return self.set_rip(addr, false);
                    }
                } else if addr > 0xffff {
                    if self.regs.get_ecx() == 0 {
                        self.regs.set_ecx(0xffffffff);
                    } else {
                        self.regs.set_ecx(self.regs.get_ecx() - 1);
                    }

                    if self.regs.get_ecx() > 0 {
                        if self.cfg.is_64bits {
                            return self.set_rip(addr, false);
                        } else {
                            return self.set_eip(addr, false);
                        }
                    }
                } else {
                    if self.regs.get_cx() == 0 {
                        self.regs.set_cx(0xffff);
                    } else {
                        self.regs.set_cx(self.regs.get_cx() - 1);
                    }

                    if self.regs.get_cx() > 0 {
                        if self.cfg.is_64bits {
                            return self.set_rip(addr, false);
                        } else {
                            return self.set_eip(addr, false);
                        }
                    }
                }
            }

            Mnemonic::Loope => {
                self.show_instruction(&self.colors.yellow, &ins);

                assert!(ins.op_count() == 1);

                let addr = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                if addr > 0xffffffff {
                    if self.regs.rcx == 0 {
                        self.regs.rcx = 0xffffffffffffffff;
                    } else {
                        self.regs.rcx -= 1;
                    }

                    if self.regs.rcx > 0 && self.flags.f_zf {
                        return self.set_rip(addr, false);
                    }
                } else if addr > 0xffff {
                    if self.regs.get_ecx() == 0 {
                        self.regs.set_ecx(0xffffffff);
                    } else {
                        self.regs.set_ecx(self.regs.get_ecx() - 1);
                    }

                    if self.regs.get_ecx() > 0 && self.flags.f_zf {
                        if self.cfg.is_64bits {
                            return self.set_rip(addr, false);
                        } else {
                            return self.set_eip(addr, false);
                        }
                    }
                } else {
                    if self.regs.get_cx() == 0 {
                        self.regs.set_cx(0xffff);
                    } else {
                        self.regs.set_cx(self.regs.get_cx() - 1);
                    }

                    if self.regs.get_cx() > 0 && self.flags.f_zf {
                        if self.cfg.is_64bits {
                            return self.set_rip(addr, false);
                        } else {
                            return self.set_eip(addr, false);
                        }
                    }
                }
            }

            Mnemonic::Loopne => {
                self.show_instruction(&self.colors.yellow, &ins);

                assert!(ins.op_count() == 1);

                let addr = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                if addr > 0xffffffff {
                    if self.regs.rcx == 0 {
                        self.regs.rcx = 0xffffffffffffffff;
                    } else {
                        self.regs.rcx -= 1;
                    }

                    if self.regs.rcx > 0 && !self.flags.f_zf {
                        return self.set_rip(addr, false);
                    }
                } else if addr > 0xffff {
                    if self.regs.get_ecx() == 0 {
                        self.regs.set_ecx(0xffffffff);
                    } else {
                        self.regs.set_ecx(self.regs.get_ecx() - 1);
                    }

                    if self.regs.get_ecx() > 0 && !self.flags.f_zf {
                        if self.cfg.is_64bits {
                            return self.set_rip(addr, false);
                        } else {
                            return self.set_eip(addr, false);
                        }
                    }
                } else {
                    if self.regs.get_cx() == 0 {
                        self.regs.set_cx(0xffff);
                    } else {
                        self.regs.set_cx(self.regs.get_cx() - 1);
                    }

                    if self.regs.get_cx() > 0 && !self.flags.f_zf {
                        if self.cfg.is_64bits {
                            return self.set_rip(addr, false);
                        } else {
                            return self.set_eip(addr, false);
                        }
                    }
                }
            }

            Mnemonic::Lea => {
                self.show_instruction(&self.colors.light_cyan, &ins);

                assert!(ins.op_count() == 2);

                let value1 = match self.get_operand_value(&ins, 1, false) {
                    Some(v) => v,
                    None => return false,
                };

                if !self.set_operand_value(&ins, 0, value1) {
                    return false;
                }
            }

            Mnemonic::Leave => {
                self.show_instruction(&self.colors.red, &ins);

                if self.cfg.is_64bits {
                    self.regs.rsp = self.regs.rbp;
                    self.regs.rbp = match self.stack_pop64(true) {
                        Some(v) => v as u64,
                        None => return false,
                    };
                } else {
                    self.regs.set_esp(self.regs.get_ebp());
                    let val = match self.stack_pop32(true) {
                        Some(v) => v as u64,
                        None => return false,
                    };
                    self.regs.set_ebp(val);
                }
            }

            Mnemonic::Int => {
                self.show_instruction(&self.colors.red, &ins);

                assert!(ins.op_count() == 1);

                let interrupt = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let handle_interrupts = match self.hook.hook_on_interrupt {
                    Some(hook_fn) => hook_fn(self, self.regs.rip, interrupt),
                    None => true,
                };

                if handle_interrupts {
                    match interrupt {
                        0x80 => {
                            self.linux = true;
                            syscall32::gateway(self);
                        }

                        0x29 => {
                            println!("int 0x21: __fastfail {}", self.regs.rcx);
                            std::process::exit(1);
                        }

                        0x03 => {
                            self.show_instruction(&self.colors.red, &ins);
                            println!("/!\\ int 0x3 sigtrap!!!!");
                            self.exception();
                            return false;
                        }

                        0xdc => {
                            println!("/!\\ direct syscall: NtAlpcSendWaitReceivePort");
                        }

                        _ => {
                            println!("unimplemented interrupt {}", interrupt);
                            return false;
                        }
                    }
                }
            }

            Mnemonic::Syscall => {
                self.show_instruction(&self.colors.red, &ins);

                syscall64::gateway(self);
            }

            Mnemonic::Std => {
                self.show_instruction(&self.colors.blue, &ins);
                self.flags.f_df = true;
            }

            Mnemonic::Stc => {
                self.show_instruction(&self.colors.blue, &ins);
                self.flags.f_cf = true;
            }

            Mnemonic::Cmc => {
                self.show_instruction(&self.colors.blue, &ins);
                self.flags.f_cf = !self.flags.f_cf;
            }

            Mnemonic::Cld => {
                self.show_instruction(&self.colors.blue, &ins);
                self.flags.f_df = false;
            }

            Mnemonic::Lodsq => {
                self.show_instruction(&self.colors.cyan, &ins);
                //TODO: crash if arrive to zero or max value

                if self.cfg.is_64bits {
                    let val = match self.maps.read_qword(self.regs.rsi) {
                        Some(v) => v,
                        None => panic!("lodsq: memory read error"),
                    };

                    self.regs.rax = val;
                    if self.flags.f_df {
                        self.regs.rsi -= 8;
                    } else {
                        self.regs.rsi += 8;
                    }
                } else {
                    unreachable!("lodsq dont exists in 32bit");
                }
            }

            Mnemonic::Lodsd => {
                self.show_instruction(&self.colors.cyan, &ins);
                //TODO: crash if arrive to zero or max value

                if self.cfg.is_64bits {
                    let val = match self.maps.read_dword(self.regs.rsi) {
                        Some(v) => v,
                        None => return false,
                    };

                    self.regs.set_eax(val as u64);
                    if self.flags.f_df {
                        self.regs.rsi -= 4;
                    } else {
                        self.regs.rsi += 4;
                    }
                } else {
                    let val = match self.maps.read_dword(self.regs.get_esi()) {
                        Some(v) => v,
                        None => return false,
                    };

                    self.regs.set_eax(val as u64);
                    if self.flags.f_df {
                        self.regs.set_esi(self.regs.get_esi() - 4);
                    } else {
                        self.regs.set_esi(self.regs.get_esi() + 4);
                    }
                }
            }

            Mnemonic::Lodsw => {
                self.show_instruction(&self.colors.cyan, &ins);
                //TODO: crash if rsi arrive to zero or max value

                if self.cfg.is_64bits {
                    let val = match self.maps.read_word(self.regs.rsi) {
                        Some(v) => v,
                        None => return false,
                    };

                    self.regs.set_ax(val as u64);
                    if self.flags.f_df {
                        self.regs.rsi -= 2;
                    } else {
                        self.regs.rsi += 2;
                    }
                } else {
                    let val = match self.maps.read_word(self.regs.get_esi()) {
                        Some(v) => v,
                        None => return false,
                    };

                    self.regs.set_ax(val as u64);
                    if self.flags.f_df {
                        self.regs.set_esi(self.regs.get_esi() - 2);
                    } else {
                        self.regs.set_esi(self.regs.get_esi() + 2);
                    }
                }
            }

            Mnemonic::Lodsb => {
                self.show_instruction(&self.colors.cyan, &ins);
                //TODO: crash if arrive to zero or max value

                if self.cfg.is_64bits {
                    let val = match self.maps.read_byte(self.regs.rsi) {
                        Some(v) => v,
                        None => {
                            println!("lodsb: memory read error");
                            self.spawn_console();
                            0
                        }
                    };

                    self.regs.set_al(val as u64);
                    if self.flags.f_df {
                        self.regs.rsi -= 1;
                    } else {
                        self.regs.rsi += 1;
                    }
                } else {
                    let val = match self.maps.read_byte(self.regs.get_esi()) {
                        Some(v) => v,
                        None => {
                            println!("lodsb: memory read error");
                            self.spawn_console();
                            0
                        }
                    };

                    self.regs.set_al(val as u64);
                    if self.flags.f_df {
                        self.regs.set_esi(self.regs.get_esi() - 1);
                    } else {
                        self.regs.set_esi(self.regs.get_esi() + 1);
                    }
                }
            }

            Mnemonic::Cbw => {
                self.show_instruction(&self.colors.green, &ins);

                let sigextend = self.regs.get_al() as u8 as i8 as i16 as u16;
                self.regs.set_ax(sigextend as u64);
            }

            Mnemonic::Cwde => {
                self.show_instruction(&self.colors.green, &ins);

                let sigextend = self.regs.get_ax() as u16 as i16 as i32 as u32;

                self.regs.set_eax(sigextend as u64);
            }

            Mnemonic::Cwd => {
                self.show_instruction(&self.colors.green, &ins);

                let sigextend = self.regs.get_ax() as u16 as i16 as i32 as u32;
                self.regs.set_ax((sigextend & 0x0000ffff) as u64);
                self.regs.set_dx(((sigextend & 0xffff0000) >> 16) as u64);
            }

            ///// FPU /////  https://github.com/radare/radare/blob/master/doc/xtra/fpu
            Mnemonic::Fninit => {
                self.fpu.clear();
            }

            Mnemonic::Finit => {
                self.fpu.clear();
            }

            Mnemonic::Ffree => {
                self.show_instruction(&self.colors.green, &ins);

                match ins.op_register(0) {
                    Register::ST0 => self.fpu.clear_st(0),
                    Register::ST1 => self.fpu.clear_st(1),
                    Register::ST2 => self.fpu.clear_st(2),
                    Register::ST3 => self.fpu.clear_st(3),
                    Register::ST4 => self.fpu.clear_st(4),
                    Register::ST5 => self.fpu.clear_st(5),
                    Register::ST6 => self.fpu.clear_st(6),
                    Register::ST7 => self.fpu.clear_st(7),
                    _ => unimplemented!("impossible case"),
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fbld => {
                self.show_instruction(&self.colors.green, &ins);

                let value = match self.get_operand_value(&ins, 0, false) {
                    Some(v) => v as u16,
                    None => return false,
                };

                //println!("{} {}", value, value as f32);
                self.fpu.set_st(0, value as f64);
            }

            Mnemonic::Fldcw => {
                self.show_instruction(&self.colors.green, &ins);

                let value = match self.get_operand_value(&ins, 0, false) {
                    Some(v) => v as u16,
                    None => return false,
                };

                self.fpu.set_ctrl(value);
            }

            Mnemonic::Fnstenv => {
                self.show_instruction(&self.colors.green, &ins);

                let addr = match self.get_operand_value(&ins, 0, false) {
                    Some(v) => v,
                    None => return false,
                };

                if self.cfg.is_64bits {
                    let env = self.fpu.get_env64();

                    for i in 0..4 {
                        self.maps.write_qword(addr + (i * 4), env[i as usize]);
                    }
                } else {
                    let env = self.fpu.get_env32();
                    for i in 0..4 {
                        self.maps.write_dword(addr + (i * 4), env[i as usize]);
                    }
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fld => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fldz => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.push(0.0);
                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fld1 => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.push(1.0);
                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fldpi => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.push(std::f64::consts::PI);
                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fldl2t => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.push(10f64.log2());
                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fldlg2 => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.push(2f64.log10());
                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fldln2 => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.push(2f64.log(std::f64::consts::E));
                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fldl2e => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.push(std::f64::consts::E.log2());
                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fst => {
                self.show_instruction(&self.colors.green, &ins);

                let res = self.fpu.get_st(0) as u64;

                if !self.set_operand_value(&ins, 0, res) {
                    return false;
                }
            }

            Mnemonic::Fsubrp => {
                self.show_instruction(&self.colors.green, &ins);

                let st0 = self.fpu.get_st(0);
                let st1 = self.fpu.get_st(1);
                let result = st1 - st0;

                self.fpu.set_st(1, result);
                self.fpu.pop();
            }

            Mnemonic::Fstp => {
                self.show_instruction(&self.colors.green, &ins);

                let res = self.fpu.get_st(0) as u64;

                if !self.set_operand_value(&ins, 0, res) {
                    return false;
                }

                self.fpu.pop();
            }

            Mnemonic::Fincstp => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.f_c1 = false;
                self.fpu.inc_top();
            }

            Mnemonic::Fild => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.dec_top();

                //C1	Set to 1 if stack overflow occurred; set to 0 otherwise.

                //println!("operands: {}", ins.op_count());
                let value1 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v as i64 as f64,
                    None => return false,
                };

                self.fpu.set_st(0, value1);
            }

            Mnemonic::Fist => {
                self.show_instruction(&self.colors.green, &ins);

                let value = self.fpu.get_st(0) as i64;
                let value2 = match self.get_operand_sz(&ins, 0) {
                    16 => value as i64 as i16 as u16 as u64,
                    32 => value as i64 as i32 as u32 as u64,
                    64 => value as i64 as u64,
                    _ => return false,
                };
                self.set_operand_value(&ins, 0, value2);
            }

            Mnemonic::Fxtract => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);
                let (mantissa, exponent) = self.fpu.frexp(st0);
                self.fpu.set_st(0, mantissa);
                self.fpu.push(exponent as f64);
            }

            Mnemonic::Fistp => {
                self.show_instruction(&self.colors.green, &ins);

                let value = self.fpu.get_st(0) as i64;
                let value2 = match self.get_operand_sz(&ins, 0) {
                    16 => value as i64 as i16 as u16 as u64,
                    32 => value as i64 as i32 as u32 as u64,
                    64 => value as i64 as u64,
                    _ => return false,
                };
                if !self.set_operand_value(&ins, 0, value2) {
                    return false;
                }

                self.fpu.pop();
                self.fpu.set_st(0, 0.0);
                self.fpu.inc_top();
            }

            Mnemonic::Fcmove => {
                self.show_instruction(&self.colors.green, &ins);

                if self.flags.f_zf {
                    match ins.op_register(0) {
                        Register::ST0 => self.fpu.move_to_st0(0),
                        Register::ST1 => self.fpu.move_to_st0(1),
                        Register::ST2 => self.fpu.move_to_st0(2),
                        Register::ST3 => self.fpu.move_to_st0(3),
                        Register::ST4 => self.fpu.move_to_st0(4),
                        Register::ST5 => self.fpu.move_to_st0(5),
                        Register::ST6 => self.fpu.move_to_st0(6),
                        Register::ST7 => self.fpu.move_to_st0(7),
                        _ => unimplemented!("impossible case"),
                    }
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fcmovb => {
                self.show_instruction(&self.colors.green, &ins);

                if self.flags.f_cf {
                    match ins.op_register(0) {
                        Register::ST0 => self.fpu.move_to_st0(0),
                        Register::ST1 => self.fpu.move_to_st0(1),
                        Register::ST2 => self.fpu.move_to_st0(2),
                        Register::ST3 => self.fpu.move_to_st0(3),
                        Register::ST4 => self.fpu.move_to_st0(4),
                        Register::ST5 => self.fpu.move_to_st0(5),
                        Register::ST6 => self.fpu.move_to_st0(6),
                        Register::ST7 => self.fpu.move_to_st0(7),
                        _ => unimplemented!("impossible case"),
                    }
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fcmovbe => {
                self.show_instruction(&self.colors.green, &ins);

                if self.flags.f_cf || self.flags.f_zf {
                    match ins.op_register(0) {
                        Register::ST0 => self.fpu.move_to_st0(0),
                        Register::ST1 => self.fpu.move_to_st0(1),
                        Register::ST2 => self.fpu.move_to_st0(2),
                        Register::ST3 => self.fpu.move_to_st0(3),
                        Register::ST4 => self.fpu.move_to_st0(4),
                        Register::ST5 => self.fpu.move_to_st0(5),
                        Register::ST6 => self.fpu.move_to_st0(6),
                        Register::ST7 => self.fpu.move_to_st0(7),
                        _ => unimplemented!("impossible case"),
                    }
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fcmovu => {
                self.show_instruction(&self.colors.green, &ins);

                if self.flags.f_pf {
                    match ins.op_register(0) {
                        Register::ST0 => self.fpu.move_to_st0(0),
                        Register::ST1 => self.fpu.move_to_st0(1),
                        Register::ST2 => self.fpu.move_to_st0(2),
                        Register::ST3 => self.fpu.move_to_st0(3),
                        Register::ST4 => self.fpu.move_to_st0(4),
                        Register::ST5 => self.fpu.move_to_st0(5),
                        Register::ST6 => self.fpu.move_to_st0(6),
                        Register::ST7 => self.fpu.move_to_st0(7),
                        _ => unimplemented!("impossible case"),
                    }
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fcmovnb => {
                self.show_instruction(&self.colors.green, &ins);

                if !self.flags.f_cf {
                    match ins.op_register(0) {
                        Register::ST0 => self.fpu.move_to_st0(0),
                        Register::ST1 => self.fpu.move_to_st0(1),
                        Register::ST2 => self.fpu.move_to_st0(2),
                        Register::ST3 => self.fpu.move_to_st0(3),
                        Register::ST4 => self.fpu.move_to_st0(4),
                        Register::ST5 => self.fpu.move_to_st0(5),
                        Register::ST6 => self.fpu.move_to_st0(6),
                        Register::ST7 => self.fpu.move_to_st0(7),
                        _ => unimplemented!("impossible case"),
                    }
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fcmovne => {
                self.show_instruction(&self.colors.green, &ins);

                if !self.flags.f_zf {
                    match ins.op_register(0) {
                        Register::ST0 => self.fpu.move_to_st0(0),
                        Register::ST1 => self.fpu.move_to_st0(1),
                        Register::ST2 => self.fpu.move_to_st0(2),
                        Register::ST3 => self.fpu.move_to_st0(3),
                        Register::ST4 => self.fpu.move_to_st0(4),
                        Register::ST5 => self.fpu.move_to_st0(5),
                        Register::ST6 => self.fpu.move_to_st0(6),
                        Register::ST7 => self.fpu.move_to_st0(7),
                        _ => unimplemented!("impossible case"),
                    }
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fcmovnbe => {
                self.show_instruction(&self.colors.green, &ins);

                if !self.flags.f_cf && !self.flags.f_zf {
                    match ins.op_register(0) {
                        Register::ST0 => self.fpu.move_to_st0(0),
                        Register::ST1 => self.fpu.move_to_st0(1),
                        Register::ST2 => self.fpu.move_to_st0(2),
                        Register::ST3 => self.fpu.move_to_st0(3),
                        Register::ST4 => self.fpu.move_to_st0(4),
                        Register::ST5 => self.fpu.move_to_st0(5),
                        Register::ST6 => self.fpu.move_to_st0(6),
                        Register::ST7 => self.fpu.move_to_st0(7),
                        _ => unimplemented!("impossible case"),
                    }
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fcmovnu => {
                self.show_instruction(&self.colors.green, &ins);

                if !self.flags.f_pf {
                    match ins.op_register(0) {
                        Register::ST0 => self.fpu.move_to_st0(0),
                        Register::ST1 => self.fpu.move_to_st0(1),
                        Register::ST2 => self.fpu.move_to_st0(2),
                        Register::ST3 => self.fpu.move_to_st0(3),
                        Register::ST4 => self.fpu.move_to_st0(4),
                        Register::ST5 => self.fpu.move_to_st0(5),
                        Register::ST6 => self.fpu.move_to_st0(6),
                        Register::ST7 => self.fpu.move_to_st0(7),
                        _ => unimplemented!("impossible case"),
                    }
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fxch => {
                self.show_instruction(&self.colors.blue, &ins);
                match ins.op_register(1) {
                    Register::ST0 => self.fpu.xchg_st(0),
                    Register::ST1 => self.fpu.xchg_st(1),
                    Register::ST2 => self.fpu.xchg_st(2),
                    Register::ST3 => self.fpu.xchg_st(3),
                    Register::ST4 => self.fpu.xchg_st(4),
                    Register::ST5 => self.fpu.xchg_st(5),
                    Register::ST6 => self.fpu.xchg_st(6),
                    Register::ST7 => self.fpu.xchg_st(7),
                    _ => unimplemented!("impossible case"),
                }

                self.fpu.set_ip(self.regs.rip);
            }

            Mnemonic::Fsqrt => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);

                self.fpu.set_st(0, st0.sqrt());
            }

            Mnemonic::Fchs => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);

                self.fpu.set_st(0, st0 * -1f64);
                self.fpu.f_c0 = false;
            }

            Mnemonic::Fptan => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);

                self.fpu.set_st(0, st0.tan());
                self.fpu.push(1.0);
            }

            Mnemonic::Fmulp => {
                self.show_instruction(&self.colors.green, &ins);
                let value0 = self.get_operand_value(&ins, 0, false).unwrap_or(0) as usize;
                let value1 = self.get_operand_value(&ins, 1, false).unwrap_or(0) as usize;
                let result = self.fpu.get_st(value1) * self.fpu.get_st(value0);

                self.fpu.set_st(value1, result);
                self.fpu.pop();
            }

            Mnemonic::Fdivp => {
                self.show_instruction(&self.colors.green, &ins);
                let value0 = self.get_operand_value(&ins, 0, false).unwrap_or(0) as usize;
                let value1 = self.get_operand_value(&ins, 1, false).unwrap_or(0) as usize;
                let result = self.fpu.get_st(value1) / self.fpu.get_st(value0);

                self.fpu.set_st(value1, result);
                self.fpu.pop();
            }

            Mnemonic::Fsubp => {
                self.show_instruction(&self.colors.green, &ins);
                let value0 = self.get_operand_value(&ins, 0, false).unwrap_or(0) as usize;
                let value1 = 0;
                let result = self.fpu.get_st(value0) - self.fpu.get_st(value1);

                self.fpu.set_st(value0, result);
                self.fpu.pop();
            }

            Mnemonic::Fsubr => {
                self.show_instruction(&self.colors.green, &ins);
                let value0 = self.get_operand_value(&ins, 0, false).unwrap_or(0) as usize;
                let value1 = self.get_operand_value(&ins, 1, false).unwrap_or(0) as usize;
                let result = self.fpu.get_st(value1) - self.fpu.get_st(value0);

                self.fpu.set_st(value1, result);
            }

            Mnemonic::Fsub => {
                self.show_instruction(&self.colors.green, &ins);
                let value0 = self.get_operand_value(&ins, 0, false).unwrap_or(0);
                let value1 = self.get_operand_value(&ins, 1, false).unwrap_or(0);
                let stA = self.fpu.get_st(value0 as usize);
                let stB = self.fpu.get_st(value1 as usize);
                self.fpu.set_st(value0 as usize, stA - stB);
            }

            Mnemonic::Fadd => {
                self.show_instruction(&self.colors.green, &ins);
                //assert!(ins.op_count() == 2); there are with 1 operand

                if ins.op_register(0) == Register::ST0 {
                    match ins.op_register(1) {
                        Register::ST0 => self.fpu.add_to_st0(0),
                        Register::ST1 => self.fpu.add_to_st0(1),
                        Register::ST2 => self.fpu.add_to_st0(2),
                        Register::ST3 => self.fpu.add_to_st0(3),
                        Register::ST4 => self.fpu.add_to_st0(4),
                        Register::ST5 => self.fpu.add_to_st0(5),
                        Register::ST6 => self.fpu.add_to_st0(6),
                        Register::ST7 => self.fpu.add_to_st0(7),
                        _ => self.fpu.add_to_st0(0),
                    }
                } else {
                    let i = match ins.op_register(0) {
                        Register::ST0 => 0,
                        Register::ST1 => 1,
                        Register::ST2 => 2,
                        Register::ST3 => 3,
                        Register::ST4 => 4,
                        Register::ST5 => 5,
                        Register::ST6 => 6,
                        Register::ST7 => 7,
                        _ => 0,
                    };

                    let j = match ins.op_register(1) {
                        Register::ST0 => 0,
                        Register::ST1 => 1,
                        Register::ST2 => 2,
                        Register::ST3 => 3,
                        Register::ST4 => 4,
                        Register::ST5 => 5,
                        Register::ST6 => 6,
                        Register::ST7 => 7,
                        _ => 0,
                    };

                    self.fpu.add(i, j);
                }
            }

            Mnemonic::Fucom => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);
                let st1 = self.fpu.get_st(1);
                self.fpu.f_c0 = st0 < st1;
                self.fpu.f_c2 = st0.is_nan() || st1.is_nan();
                self.fpu.f_c3 = st0 == st1;
            }

            Mnemonic::F2xm1 => {
                self.show_instruction(&self.colors.green, &ins);

                let st0 = self.fpu.get_st(0);
                let result = (2.0f64.powf(st0)) - 1.0;
                self.fpu.set_st(0, result);
            }

            Mnemonic::Fyl2x => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.fyl2x();
            }

            Mnemonic::Fyl2xp1 => {
                self.show_instruction(&self.colors.green, &ins);

                self.fpu.fyl2xp1();
            }

            // end fpu
            Mnemonic::Popf => {
                self.show_instruction(&self.colors.blue, &ins);

                let flags: u16 = match self.maps.read_word(self.regs.rsp) {
                    Some(v) => v,
                    None => {
                        eprintln!("popf cannot read the stack");
                        self.exception();
                        return false;
                    }
                };

                let flags2: u32 = (self.flags.dump() & 0xffff0000) + (flags as u32);
                self.flags.load(flags2);
                self.regs.rsp += 2;
            }

            Mnemonic::Popfd => {
                self.show_instruction(&self.colors.blue, &ins);

                let flags = match self.stack_pop32(true) {
                    Some(v) => v,
                    None => return false,
                };
                self.flags.load(flags);
            }

            Mnemonic::Popfq => {
                self.show_instruction(&self.colors.blue, &ins);

                let eflags = match self.stack_pop64(true) {
                    Some(v) => v as u32,
                    None => return false,
                };
                self.flags.load(eflags);
            }

            Mnemonic::Daa => {
                self.show_instruction(&self.colors.green, &ins);

                let old_al = self.regs.get_al();
                let old_cf = self.flags.f_cf;
                self.flags.f_cf = false;

                if (self.regs.get_al() & 0x0f > 9) || self.flags.f_af {
                    let sum = self.regs.get_al() + 6;
                    self.regs.set_al(sum & 0xff);
                    if sum > 0xff {
                        self.flags.f_cf = true;
                    } else {
                        self.flags.f_cf = old_cf;
                    }

                    self.flags.f_af = true;
                } else {
                    self.flags.f_af = false;
                }

                if old_al > 0x99 || old_cf {
                    self.regs.set_al(self.regs.get_al() + 0x60);
                    self.flags.f_cf = true;
                } else {
                    self.flags.f_cf = false;
                }
            }

            Mnemonic::Shld => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let counter = match self.get_operand_value(&ins, 2, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);

                if value0 == 0xde2f && value1 == 0x4239 && counter == 0x3c && sz == 16 {
                    if self.cfg.verbose >= 1 {
                        println!("/!\\ shld undefined behaviour");
                    }
                    let result = 0x9de2;
                    // TODO: flags?
                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                } else {
                    let (result, new_flags) =
                        inline::shld(value0, value1, counter, sz, self.flags.dump());
                    self.flags.load(new_flags);
                    if !self.set_operand_value(&ins, 0, result) {
                        return false;
                    }
                }
            }

            Mnemonic::Shrd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let counter = match self.get_operand_value(&ins, 2, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0);
                let (result, new_flags) =
                    inline::shrd(value0, value1, counter, sz, self.flags.dump());
                self.flags.load(new_flags);

                //println!("0x{:x} SHRD 0x{:x}, 0x{:x}, 0x{:x} = 0x{:x}", ins.ip32(), value0, value1, counter, result);
                /*
                if self.cfg.test_mode { //&& !undef {
                    if result != inline::shrd(value0, value1, counter, sz) {
                        panic!("SHRD{} 0x{:x} should be 0x{:x}", sz, result, inline::shrd(value0, value1, counter, sz));
                    }
                }*/

                if !self.set_operand_value(&ins, 0, result) {
                    return false;
                }
            }

            Mnemonic::Sysenter => {
                if self.cfg.is_64bits {
                    unimplemented!("ntapi64 not implemented yet");
                } else {
                    ntapi32::gateway(self.regs.get_eax(), self.regs.get_edx(), self);
                }
            }

            //// SSE XMM ////
            // scalar: only gets the less significative part.
            // scalar simple: only 32b less significative part.
            // scalar double: only 54b less significative part.
            // packed: compute all parts.
            // packed double
            Mnemonic::Pcmpeqd => {
                self.show_instruction(&self.colors.green, &ins);
                if self.get_operand_sz(&ins, 0) != 128 || self.get_operand_sz(&ins, 1) != 128 {
                    println!("unimplemented");
                    return false;
                }

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let mut result = 0u128;

                for i in 0..4 {
                    let mask = 0xFFFFFFFFu128;
                    let shift = i * 32;

                    let dword0 = (value0 >> shift) & mask;
                    let dword1 = (value1 >> shift) & mask;

                    if dword0 == dword1 {
                        result |= mask << shift;
                    }
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Psubusb => {
                self.show_instruction(&self.colors.green, &ins);
                if self.get_operand_sz(&ins, 0) != 128 || self.get_operand_sz(&ins, 1) != 128 {
                    println!("unimplemented");
                    return false;
                }

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let mut result = 0u128;
                for i in 0..16 {
                    let byte0 = ((value0 >> (i * 8)) & 0xFF) as u8;
                    let byte1 = ((value1 >> (i * 8)) & 0xFF) as u8;
                    let res_byte = byte0.saturating_sub(byte1);

                    result |= (res_byte as u128) << (i * 8);
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Punpckhbw => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let bytes0 = value0.to_le_bytes();
                let bytes1 = value1.to_le_bytes();

                let mut result_bytes = [0u8; 16];
                result_bytes[0] = bytes0[8];
                result_bytes[1] = bytes1[8];
                result_bytes[2] = bytes0[9];
                result_bytes[3] = bytes1[9];
                result_bytes[4] = bytes0[10];
                result_bytes[5] = bytes1[10];
                result_bytes[6] = bytes0[11];
                result_bytes[7] = bytes1[11];
                result_bytes[8] = bytes0[12];
                result_bytes[9] = bytes1[12];
                result_bytes[10] = bytes0[13];
                result_bytes[11] = bytes1[13];
                result_bytes[12] = bytes0[14];
                result_bytes[13] = bytes1[14];
                result_bytes[14] = bytes0[15];
                result_bytes[15] = bytes1[15];

                let result = u128::from_le_bytes(result_bytes);
                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Pand => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value1");
                        return false;
                    }
                };

                let result: u128 = value0 & value1;
                self.flags.calc_flags(result as u64, 32);

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Por => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value1");
                        return false;
                    }
                };

                let result: u128 = value0 | value1;
                self.flags.calc_flags(result as u64, 32);

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Pxor => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 2);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value1");
                        return false;
                    }
                };

                let result: u128 = value0 ^ value1;
                self.flags.calc_flags(result as u64, 32);

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Punpcklbw => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 2);
                let sz0 = self.get_operand_sz(&ins, 0);
                if sz0 == 128 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value0");
                            return false;
                        }
                    };
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };

                    let mut result: u128 = 0;
                    let mask_byte = 0xff;

                    for i in 0..8 {
                        let byte_value0 = (value0 >> (8 * i)) & mask_byte;
                        let byte_value1 = (value1 >> (8 * i)) & mask_byte;

                        result |= byte_value0 << (16 * i);
                        result |= byte_value1 << (16 * i + 8);
                    }

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else {
                    unimplemented!("unimplemented size");
                }
            }

            Mnemonic::Punpcklwd => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 2);
                let sz0 = self.get_operand_sz(&ins, 0);
                if sz0 == 128 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value0");
                            return false;
                        }
                    };
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };

                    let mut result = 0u128;
                    for i in 0..2 {
                        let word_value0 = (value0 >> (i * 16)) & 0xFFFF;
                        let word_value1 = (value1 >> (i * 16)) & 0xFFFF;
                        result |= word_value0 << (i * 32);
                        result |= word_value1 << (i * 32 + 16);
                    }

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else {
                    unimplemented!("unimplemented size");
                }
            }

            Mnemonic::Xorps => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value1");
                        return false;
                    }
                };

                let a: u128 = (value0 & 0xffffffff) ^ (value1 & 0xffffffff);
                let b: u128 = (value0 & 0xffffffff_00000000) ^ (value1 & 0xffffffff_00000000);
                let c: u128 = (value0 & 0xffffffff_00000000_00000000)
                    ^ (value1 & 0xffffffff_00000000_00000000);
                let d: u128 = (value0 & 0xffffffff_00000000_00000000_00000000)
                    ^ (value1 & 0xffffffff_00000000_00000000_00000000);

                let result: u128 = a | b | c | d;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Xorpd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value1");
                        return false;
                    }
                };

                let a: u128 = (value0 & 0xffffffff_ffffffff) ^ (value1 & 0xffffffff_ffffffff);
                let b: u128 = (value0 & 0xffffffff_ffffffff_00000000_00000000)
                    ^ (value1 & 0xffffffff_ffffffff_00000000_00000000);
                let result: u128 = a | b;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            /*            Mnemonic::Psubb
            | Mnemonic::Psubw
            | Mnemonic::Psubd
            | Mnemonic::Psubq
            | Mnemonic::Psubsb
            | Mnemonic::Psubsw
            | Mnemonic::Psubusb
            | Mnemonic::Psubusw => {*/
            Mnemonic::Psubb => {
                self.show_instruction(&self.colors.cyan, &ins);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz0 == 128 && sz1 == 128 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };

                    let mut result = 0u128;
                    for i in 0..16 {
                        let byte0 = (value0 >> (8 * i)) & 0xFF;
                        let byte1 = (value1 >> (8 * i)) & 0xFF;
                        let res_byte = byte0.wrapping_sub(byte1);
                        result |= res_byte << (8 * i);
                    }

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else {
                    unimplemented!();
                }
            }

            Mnemonic::Psubw => {
                self.show_instruction(&self.colors.cyan, &ins);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz0 == 128 && sz1 == 128 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };

                    let mut result = 0u128;
                    for i in 0..8 {
                        let word0 = (value0 >> (16 * i)) & 0xFFFF;
                        let word1 = (value1 >> (16 * i)) & 0xFFFF;
                        let res_word = word0.wrapping_sub(word1);
                        result |= res_word << (16 * i);
                    }

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else {
                    unimplemented!();
                }
            }

            Mnemonic::Psubd => {
                self.show_instruction(&self.colors.cyan, &ins);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz0 == 128 && sz1 == 128 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };

                    let mut result = 0u128;
                    for i in 0..4 {
                        let dword0 = (value0 >> (32 * i)) & 0xFFFFFFFF;
                        let dword1 = (value1 >> (32 * i)) & 0xFFFFFFFF;
                        let res_dword = dword0.wrapping_sub(dword1);
                        result |= res_dword << (32 * i);
                    }

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else {
                    unimplemented!();
                }
            }

            Mnemonic::Psubq => {
                self.show_instruction(&self.colors.cyan, &ins);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz0 == 128 && sz1 == 128 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };

                    let mut result = 0u128;
                    for i in 0..2 {
                        let qword0 = (value0 >> (64 * i)) & 0xFFFFFFFFFFFFFFFF;
                        let qword1 = (value1 >> (64 * i)) & 0xFFFFFFFFFFFFFFFF;
                        let res_qword = qword0.wrapping_sub(qword1);
                        result |= res_qword << (64 * i);
                    }

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else {
                    unimplemented!();
                }
            }

            // movlpd: packed double, movlps: packed simple, cvtsi2sd: int to scalar double 32b to 64b,
            // cvtsi2ss: int to scalar single copy 32b to 32b, movd: doubleword move
            Mnemonic::Movhpd => {
                // we keep the high part of xmm destination

                self.show_instruction(&self.colors.cyan, &ins);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz0 == 128 && sz1 == 128 {
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    self.set_operand_xmm_value_128(&ins, 0, value1);
                } else if sz0 == 128 && sz1 == 32 {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    unimplemented!("mov 32bits to the 64bits highest part of the xmm1 u128");
                    //self.set_operand_xmm_value_128(&ins, 0, value1 as u128);
                } else if sz0 == 32 && sz1 == 128 {
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    unimplemented!("mov 32bits to the 64bits highest part of the xmm1 u128");
                    //self.set_operand_value(&ins, 0, value1 as u64);
                } else if sz0 == 128 && sz1 == 64 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, false) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm address value1");
                            return false;
                        }
                    };
                    let addr = match self.get_operand_value(&ins, 1, false) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm address value1");
                            return false;
                        }
                    };
                    let value1 = match self.maps.read_qword(addr) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm qword value1");
                            return false;
                        }
                    };

                    let result: u128 = (value1 as u128) << 64 | value0 & 0xffffffffffffffff;

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else if sz0 == 64 && sz1 == 128 {
                    let mut value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    value1 = value1 >> 64;

                    self.set_operand_value(&ins, 0, value1 as u64);
                } else {
                    println!("SSE with other size combinations sz0:{} sz1:{}", sz0, sz1);
                    return false;
                }
            }

            Mnemonic::Movlpd | Mnemonic::Movlps | Mnemonic::Cvtsi2sd | Mnemonic::Cvtsi2ss => {
                // we keep the high part of xmm destination

                self.show_instruction(&self.colors.cyan, &ins);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz0 == 128 && sz1 == 128 {
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    self.set_operand_xmm_value_128(&ins, 0, value1);
                } else if sz0 == 128 && sz1 == 32 {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    self.set_operand_xmm_value_128(&ins, 0, value1 as u128);
                } else if sz0 == 32 && sz1 == 128 {
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    self.set_operand_value(&ins, 0, value1 as u64);
                } else if sz0 == 128 && sz1 == 64 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, false) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm address value1");
                            return false;
                        }
                    };
                    let addr = match self.get_operand_value(&ins, 1, false) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm address value1");
                            return false;
                        }
                    };
                    let value1 = match self.maps.read_qword(addr) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm qword value1");
                            return false;
                        }
                    };

                    let mask: u128 = 0xFFFFFFFFFFFFFFFF_0000000000000000;
                    let result: u128 = (value0 & mask) | (value1 as u128);

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else if sz0 == 64 && sz1 == 128 {
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    self.set_operand_value(&ins, 0, value1 as u64);
                } else {
                    println!("SSE with other size combinations sz0:{} sz1:{}", sz0, sz1);
                    return false;
                }
            }

            Mnemonic::Movhps => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz0 == 128 && sz1 == 64 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value0");
                            return false;
                        }
                    };

                    let value1 = match self.get_operand_value(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting value1");
                            return false;
                        }
                    };

                    let lower_value0 = value0 & 0x00000000_FFFFFFFF_00000000_FFFFFFFF;
                    let upper_value1 = (value1 as u128) << 64;
                    let result = lower_value0 | upper_value1;

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else if sz0 == 64 && sz1 == 128 {
                    let value1 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };

                    let result = (value1 >> 64) as u64;

                    self.set_operand_value(&ins, 0, result);
                } else if sz0 == 128 && sz1 == 32 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value0");
                            return false;
                        }
                    };

                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => (v & 0xffffffff) as u32,
                        None => {
                            println!("error getting value1");
                            return false;
                        }
                    };

                    let lower_value0 = value0 & 0x00000000_FFFFFFFF_FFFFFFFF_FFFFFFFF;
                    let upper_value1 = (value1 as u128) << 96;
                    let result = lower_value0 | upper_value1;

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else {
                    unimplemented!("case of movhps unimplemented {} {}", sz0, sz1);
                }
            }

            Mnemonic::Punpcklqdq => {
                self.show_instruction(&self.colors.green, &ins);
                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz0 == 128 && sz1 == 128 {
                    let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value0");
                            return false;
                        }
                    };

                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => (v & 0xffffffff) as u32,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    let value0_low_qword = value0 as u64;
                    let value1_low_qword = value1 as u64;
                    let result = ((value0_low_qword as u128) << 64) | (value1_low_qword as u128);

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else {
                    println!("unimplemented case punpcklqdq {} {}", sz0, sz1);
                    return false;
                }
            }

            Mnemonic::Movq => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);
                let value1: u128;

                if sz1 == 128 {
                    value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                } else if sz1 < 128 {
                    value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v as u128,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                } else {
                    unimplemented!("ymm zmm unimplemented on movq");
                }

                if sz0 == 128 {
                    self.set_operand_xmm_value_128(&ins, 0, value1);
                } else if sz0 < 128 {
                    self.set_operand_value(&ins, 0, value1 as u64);
                } else {
                    unimplemented!("ymm zmm unimplemented on movq");
                }
            }

            Mnemonic::Punpckhdq => {
                self.show_instruction(&self.colors.cyan, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value1");
                        return false;
                    }
                };

                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value1");
                        return false;
                    }
                };

                let dword0_0 = (value0 >> 96) as u32;
                let dword0_1 = (value0 >> 64) as u32;
                let dword1_0 = (value1 >> 96) as u32;
                let dword1_1 = (value1 >> 64) as u32;

                let result: u128 = ((dword0_0 as u128) << 96)
                    | ((dword1_0 as u128) << 64)
                    | ((dword0_1 as u128) << 32)
                    | (dword1_1 as u128);

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Punpckldq => {
                self.show_instruction(&self.colors.cyan, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value1");
                        return false;
                    }
                };

                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting xmm value1");
                        return false;
                    }
                };

                let dword0_0 = (value0 & 0xFFFFFFFF) as u32;
                let dword0_1 = ((value0 >> 32) & 0xFFFFFFFF) as u32;
                let dword1_0 = (value1 & 0xFFFFFFFF) as u32;
                let dword1_1 = ((value1 >> 32) & 0xFFFFFFFF) as u32;

                let result: u128 = ((dword0_0 as u128) << 96)
                    | ((dword1_0 as u128) << 64)
                    | ((dword0_1 as u128) << 32)
                    | (dword1_1 as u128);

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Movd => {
                // the high part is cleared to zero

                self.show_instruction(&self.colors.cyan, &ins);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz0 == 128 && sz1 == 128 {
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    self.set_operand_xmm_value_128(&ins, 0, value1);
                } else if sz0 == 128 && sz1 == 32 {
                    let value1 = match self.get_operand_value(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    self.set_operand_xmm_value_128(&ins, 0, value1 as u128);
                } else if sz0 == 32 && sz1 == 128 {
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    self.set_operand_value(&ins, 0, value1 as u64);
                } else if sz0 == 128 && sz1 == 64 {
                    let addr = match self.get_operand_value(&ins, 1, false) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm address value1");
                            return false;
                        }
                    };
                    let value1 = match self.maps.read_qword(addr) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm qword value1");
                            return false;
                        }
                    };

                    self.set_operand_xmm_value_128(&ins, 0, value1 as u128);
                } else if sz0 == 64 && sz1 == 128 {
                    let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    self.set_operand_value(&ins, 0, value1 as u64);
                } else {
                    println!("SSE with other size combinations sz0:{} sz1:{}", sz0, sz1);
                    return false;
                }
            }

            Mnemonic::Movdqa => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 2);

                let sz0 = self.get_operand_sz(&ins, 0);
                let sz1 = self.get_operand_sz(&ins, 1);

                if sz0 == 32 && sz1 == 128 {
                    let xmm = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };
                    let addr = match self.get_operand_value(&ins, 0, false) {
                        Some(v) => v,
                        None => {
                            println!("error getting address value0");
                            return false;
                        }
                    };
                    //println!("addr: 0x{:x} value: 0x{:x}", addr, xmm);
                    self.maps.write_dword(
                        addr,
                        ((xmm & 0xffffffff_00000000_00000000_00000000) >> (12 * 8)) as u32,
                    );
                    self.maps.write_dword(
                        addr + 4,
                        ((xmm & 0xffffffff_00000000_00000000) >> (8 * 8)) as u32,
                    );
                    self.maps
                        .write_dword(addr + 8, ((xmm & 0xffffffff_00000000) >> (4 * 8)) as u32);
                    self.maps.write_dword(addr + 12, (xmm & 0xffffffff) as u32);
                } else if sz0 == 128 && sz1 == 32 {
                    let addr = match self.get_operand_value(&ins, 1, false) {
                        Some(v) => v,
                        None => {
                            println!("error reading address value1");
                            return false;
                        }
                    };

                    let bytes = self.maps.read_bytes(addr, 16);
                    if bytes.len() != 16 {
                        println!("error reading 16 bytes");
                        return false;
                    }

                    let result = u128::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7], bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13],
                        bytes[14], bytes[15],
                    ]);

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else if sz0 == 128 && sz1 == 128 {
                    let xmm = match self.get_operand_xmm_value_128(&ins, 1, true) {
                        Some(v) => v,
                        None => {
                            println!("error getting xmm value1");
                            return false;
                        }
                    };

                    self.set_operand_xmm_value_128(&ins, 0, xmm);
                } else {
                    println!("sz0: {}  sz1: {}\n", sz0, sz1);
                    unimplemented!("movdqa");
                }
            }

            Mnemonic::Andpd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let result: u128 = value0 & value1;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Orpd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let result: u128 = value0 | value1;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Addps => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let a: u128 = (value0 & 0xffffffff) + (value1 & 0xffffffff);
                let b: u128 = (value0 & 0xffffffff_00000000) + (value1 & 0xffffffff_00000000);
                let c: u128 = (value0 & 0xffffffff_00000000_00000000)
                    + (value1 & 0xffffffff_00000000_00000000);
                let d: u128 = (value0 & 0xffffffff_00000000_00000000_00000000)
                    + (value1 & 0xffffffff_00000000_00000000_00000000);

                let result: u128 = a | b | c | d;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Addpd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let a: u128 = (value0 & 0xffffffff_ffffffff) + (value1 & 0xffffffff_ffffffff);
                let b: u128 = (value0 & 0xffffffff_ffffffff_00000000_00000000)
                    + (value1 & 0xffffffff_ffffffff_00000000_00000000);
                let result: u128 = a | b;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Addsd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let result: u64 = value0 as u64 + value1 as u64;
                let r128: u128 = (value0 & 0xffffffffffffffff0000000000000000) + result as u128;
                self.set_operand_xmm_value_128(&ins, 0, r128);
            }

            Mnemonic::Addss => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let result: u32 = value0 as u32 + value1 as u32;
                let r128: u128 = (value0 & 0xffffffffffffffffffffffff00000000) + result as u128;
                self.set_operand_xmm_value_128(&ins, 0, r128);
            }

            Mnemonic::Subps => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let a: u128 = (value0 & 0xffffffff) - (value1 & 0xffffffff);
                let b: u128 = (value0 & 0xffffffff_00000000) - (value1 & 0xffffffff_00000000);
                let c: u128 = (value0 & 0xffffffff_00000000_00000000)
                    - (value1 & 0xffffffff_00000000_00000000);
                let d: u128 = (value0 & 0xffffffff_00000000_00000000_00000000)
                    - (value1 & 0xffffffff_00000000_00000000_00000000);

                let result: u128 = a | b | c | d;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Subpd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let a: u128 = (value0 & 0xffffffff_ffffffff) - (value1 & 0xffffffff_ffffffff);
                let b: u128 = (value0 & 0xffffffff_ffffffff_00000000_00000000)
                    - (value1 & 0xffffffff_ffffffff_00000000_00000000);
                let result: u128 = a | b;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Subsd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let result: u64 = value0 as u64 - value1 as u64;
                let r128: u128 = (value0 & 0xffffffffffffffff0000000000000000) + result as u128;
                self.set_operand_xmm_value_128(&ins, 0, r128);
            }

            Mnemonic::Subss => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let result: u32 = value0 as u32 - value1 as u32;
                let r128: u128 = (value0 & 0xffffffffffffffffffffffff00000000) + result as u128;
                self.set_operand_xmm_value_128(&ins, 0, r128);
            }

            Mnemonic::Mulpd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let left: u128 = ((value0 & 0xffffffffffffffff0000000000000000) >> 64)
                    * ((value1 & 0xffffffffffffffff0000000000000000) >> 64);
                let right: u128 = (value0 & 0xffffffffffffffff) * (value1 & 0xffffffffffffffff);
                let result: u128 = left << 64 | right;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Mulps => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let a: u128 = (value0 & 0xffffffff) * (value1 & 0xffffffff);
                let b: u128 = (value0 & 0xffffffff00000000) * (value1 & 0xffffffff00000000);
                let c: u128 =
                    (value0 & 0xffffffff0000000000000000) * (value1 & 0xffffffff0000000000000000);
                let d: u128 = (value0 & 0xffffffff000000000000000000000000)
                    * (value1 & 0xffffffff000000000000000000000000);

                let result: u128 = a | b | c | d;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Mulsd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let result: u64 = value0 as u64 * value1 as u64;
                let r128: u128 = (value0 & 0xffffffffffffffff0000000000000000) + result as u128;
                self.set_operand_xmm_value_128(&ins, 0, r128);
            }

            Mnemonic::Mulss => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };

                let result: u32 = value0 as u32 * value1 as u32;
                let r128: u128 = (value0 & 0xffffffffffffffffffffffff00000000) + result as u128;
                self.set_operand_xmm_value_128(&ins, 0, r128);
            }

            Mnemonic::Packsswb => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };
                let mut result: u128;

                result = (value0 & 0xffff) as u16 as i16 as i8 as u8 as u128;
                result |= (((value0 & 0xffff0000) >> 16) as u16 as i16 as i8 as u8 as u128) << 8;
                result |=
                    (((value0 & 0xffff00000000) >> 32) as u16 as i16 as i8 as u8 as u128) << 16;
                result |=
                    (((value0 & 0xffff000000000000) >> 48) as u16 as i16 as i8 as u8 as u128) << 24;
                result |= (((value0 & 0xffff0000000000000000) >> 64) as u16 as i16 as i8 as u8
                    as u128)
                    << 32;
                result |= (((value0 & 0xffff00000000000000000000) >> 80) as u16 as i16 as i8 as u8
                    as u128)
                    << 40;
                result |= (((value0 & 0xffff000000000000000000000000) >> 96) as u16 as i16 as i8
                    as u8 as u128)
                    << 48;
                result |= (((value0 & 0xffff0000000000000000000000000000) >> 112) as u16 as i16
                    as i8 as u8 as u128)
                    << 56;
                result |= ((value1 & 0xffff) as u16 as i16 as i8 as u8 as u128) << 64;
                result |= (((value1 & 0xffff0000) >> 16) as u16 as i16 as i8 as u8 as u128) << 72;
                result |=
                    (((value1 & 0xffff00000000) >> 32) as u16 as i16 as i8 as u8 as u128) << 80;
                result |=
                    (((value1 & 0xffff000000000000) >> 48) as u16 as i16 as i8 as u8 as u128) << 88;
                result |= (((value1 & 0xffff0000000000000000) >> 64) as u16 as i16 as i8 as u8
                    as u128)
                    << 96;
                result |= (((value1 & 0xffff00000000000000000000) >> 80) as u16 as i16 as i8 as u8
                    as u128)
                    << 104;
                result |= (((value1 & 0xffff000000000000000000000000) >> 96) as u16 as i16 as i8
                    as u8 as u128)
                    << 112;
                result |= (((value1 & 0xffff0000000000000000000000000000) >> 112) as u16 as i16
                    as i8 as u8 as u128)
                    << 120;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Packssdw => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };
                let mut result: u128;

                result = (value0 & 0xffffffff) as u32 as i32 as i16 as u16 as u128;
                result |= (((value0 & 0xffffffff00000000) >> 32) as u32 as i32 as i16 as u16
                    as u128)
                    << 16;
                result |= (((value0 & 0xffffffff0000000000000000) >> 64) as u32 as i32 as i16 as u16
                    as u128)
                    << 32;
                result |= (((value0 & 0xffffffff000000000000000000000000) >> 96) as u32 as i32
                    as i16 as u16 as u128)
                    << 48;
                result |= ((value1 & 0xffffffff) as u32 as i32 as i16 as u16 as u128) << 64;
                result |= (((value1 & 0xffffffff00000000) >> 32) as u32 as i32 as i16 as u16
                    as u128)
                    << 80;
                result |= (((value1 & 0xffffffff0000000000000000) >> 64) as u32 as i32 as i16 as u16
                    as u128)
                    << 96;
                result |= (((value1 & 0xffffffff000000000000000000000000) >> 96) as u32 as i32
                    as i16 as u16 as u128)
                    << 112;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Psrldq => {
                self.show_instruction(&self.colors.green, &ins);

                if ins.op_count() == 2 {
                    let sz0 = self.get_operand_sz(&ins, 0);

                    if sz0 == 128 {
                        let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                            Some(v) => v,
                            None => {
                                println!("error getting value0");
                                return false;
                            }
                        };
                        let mut value1 = match self.get_operand_value(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error getting value1");
                                return false;
                            }
                        };

                        let result: u128;
                        if value1 > 15 {
                            value1 = 16;
                        }

                        result = value0 >> (value1 * 8);

                        self.set_operand_xmm_value_128(&ins, 0, result);
                    } else {
                        unimplemented!("size unimplemented");
                    }
                } else if ins.op_count() == 3 {
                    let sz0 = self.get_operand_sz(&ins, 0);

                    if sz0 == 128 {
                        let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error getting value0");
                                return false;
                            }
                        };
                        let mut value2 = match self.get_operand_value(&ins, 2, true) {
                            Some(v) => v,
                            None => {
                                println!("error getting value1");
                                return false;
                            }
                        };

                        let result: u128;
                        if value2 > 15 {
                            value2 = 16;
                        }

                        result = value1 >> (value2 * 8);

                        self.set_operand_xmm_value_128(&ins, 0, result);
                    } else {
                        unimplemented!("size unimplemented");
                    }
                } else {
                    unreachable!();
                }
            }

            Mnemonic::Pslld => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let shift_amount =
                    self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0) as u32;

                let mut result = 0u128;

                for i in 0..4 {
                    let mask = 0xFFFFFFFFu128;
                    let shift = i * 32;

                    let dword = ((value0 >> shift) & mask) as u32;
                    let shifted = dword.wrapping_shl(shift_amount);

                    result |= (shifted as u128 & mask) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Pslldq => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let shift_amount =
                    self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0) as u32;
                let byte_shift = (shift_amount % 16) * 8; // Desplazamiento en bits

                let result = if byte_shift < 128 {
                    value0 << byte_shift
                } else {
                    0u128
                };

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Psllq => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let shift_amount =
                    self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0) as u32;

                let mut result = 0u128;

                for i in 0..2 {
                    let mask = 0xFFFFFFFFFFFFFFFFu128;
                    let shift = i * 64;

                    let qword = ((value0 >> shift) & mask) as u64;
                    let shifted = qword.wrapping_shl(shift_amount);

                    result |= (shifted as u128 & mask) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Psllw => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };
                let mut result: u128;

                if value1 > 15 {
                    result = value0 & 0xffffffffffffffff_0000000000000000;
                } else {
                    result = (((value0 & 0xffff) as u16) << value1) as u128;
                    result |= (((((value0 & 0xffff0000) >> 16) as u16) << value1) as u128) << 16;
                    result |=
                        (((((value0 & 0xffff00000000) >> 32) as u16) << value1) as u128) << 32;
                    result |=
                        (((((value0 & 0xffff000000000000) >> 48) as u16) << value1) as u128) << 48;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Paddsw => {
                self.show_instruction(&self.colors.green, &ins);
                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let mut result = 0u128;

                for i in 0..8 {
                    let mask = 0xFFFFu128;
                    let shift = i * 16;

                    let word0 = ((value0 >> shift) & mask) as i16;
                    let word1 = ((value1 >> shift) & mask) as i16;

                    let sum = word0.saturating_add(word1);

                    result |= (sum as u128 & mask) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Paddsb => {
                self.show_instruction(&self.colors.green, &ins);
                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let mut result = 0u128;

                for i in 0..16 {
                    let mask = 0xFFu128;
                    let shift = i * 8;
                    let byte0 = ((value0 >> shift) & mask) as i8;
                    let byte1 = ((value1 >> shift) & mask) as i8;
                    let sum = byte0.saturating_add(byte1);

                    result |= (sum as u128 & mask) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Psrad => {
                self.show_instruction(&self.colors.green, &ins);
                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let mut result = 0u128;
                let shift_amount = (value1 & 0xFF) as u32;

                for i in 0..4 {
                    let mask = 0xFFFFFFFFu128;
                    let shift = i * 32;
                    let dword = ((value0 >> shift) & mask) as i32;
                    let shifted = dword >> shift_amount;

                    result |= (shifted as u128 & mask) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Paddusb | Mnemonic::Paddb => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };
                let sz = self.get_operand_sz(&ins, 0);
                let mut result: u128;

                if sz == 64 {
                    result = ((value0 & 0xff) as u8 + (value1 & 0xff) as u8) as u128;
                    result |= ((((value0 & 0xff00) >> 8) as u8 + ((value1 & 0xff00) >> 8) as u8)
                        as u128)
                        << 8;
                    result |= ((((value0 & 0xff0000) >> 16) as u8
                        + ((value1 & 0xff0000) >> 16) as u8)
                        as u128)
                        << 16;
                    result |= ((((value0 & 0xff000000) >> 24) as u8
                        + ((value1 & 0xff000000) >> 24) as u8)
                        as u128)
                        << 24;
                    result |= ((((value0 & 0xff00000000) >> 32) as u8
                        + ((value1 & 0xff00000000) >> 32) as u8)
                        as u128)
                        << 32;
                    result |= ((((value0 & 0xff0000000000) >> 40) as u8
                        + ((value1 & 0xff0000000000) >> 40) as u8)
                        as u128)
                        << 40;
                    result |= ((((value0 & 0xff000000000000) >> 48) as u8
                        + ((value1 & 0xff000000000000) >> 48) as u8)
                        as u128)
                        << 48;
                    result |= ((((value0 & 0xff00000000000000) >> 56) as u8
                        + ((value1 & 0xff00000000000000) >> 56) as u8)
                        as u128)
                        << 56;
                } else if sz == 128 {
                    result = ((value0 & 0xff) as u8 + (value1 & 0xff) as u8) as u128;
                    result |= ((((value0 & 0xff00) >> 8) as u8 + ((value1 & 0xff00) >> 8) as u8)
                        as u128)
                        << 8;
                    result |= ((((value0 & 0xff0000) >> 16) as u8
                        + ((value1 & 0xff0000) >> 16) as u8)
                        as u128)
                        << 16;
                    result |= ((((value0 & 0xff000000) >> 24) as u8
                        + ((value1 & 0xff000000) >> 24) as u8)
                        as u128)
                        << 24;
                    result |= ((((value0 & 0xff00000000) >> 32) as u8
                        + ((value1 & 0xff00000000) >> 32) as u8)
                        as u128)
                        << 32;
                    result |= ((((value0 & 0xff0000000000) >> 40) as u8
                        + ((value1 & 0xff0000000000) >> 40) as u8)
                        as u128)
                        << 40;
                    result |= ((((value0 & 0xff000000000000) >> 48) as u8
                        + ((value1 & 0xff000000000000) >> 48) as u8)
                        as u128)
                        << 48;
                    result |= ((((value0 & 0xff00000000000000) >> 56) as u8
                        + ((value1 & 0xff00000000000000) >> 56) as u8)
                        as u128)
                        << 56;

                    result |= ((((value0 & 0xff_0000000000000000) >> 64) as u8
                        + ((value1 & 0xff_0000000000000000) >> 64) as u8)
                        as u128)
                        << 64;
                    result |= ((((value0 & 0xff00_0000000000000000) >> 72) as u8
                        + ((value1 & 0xff00_0000000000000000) >> 72) as u8)
                        as u128)
                        << 72;
                    result |= ((((value0 & 0xff0000_0000000000000000) >> 80) as u8
                        + ((value1 & 0xff0000_0000000000000000) >> 80) as u8)
                        as u128)
                        << 80;
                    result |= ((((value0 & 0xff000000_0000000000000000) >> 88) as u8
                        + ((value1 & 0xff000000_0000000000000000) >> 88) as u8)
                        as u128)
                        << 88;
                    result |= ((((value0 & 0xff00000000_0000000000000000) >> 96) as u8
                        + ((value1 & 0xff00000000_0000000000000000) >> 96) as u8)
                        as u128)
                        << 96;
                    result |= ((((value0 & 0xff0000000000_0000000000000000) >> 104) as u8
                        + ((value1 & 0xff0000000000_0000000000000000) >> 104) as u8)
                        as u128)
                        << 104;
                    result |= ((((value0 & 0xff000000000000_0000000000000000) >> 112) as u8
                        + ((value1 & 0xff000000000000_0000000000000000) >> 112) as u8)
                        as u128)
                        << 112;
                    result |= ((((value0 & 0xff00000000000000_0000000000000000) >> 120) as u8
                        + ((value1 & 0xff00000000000000_0000000000000000) >> 120) as u8)
                        as u128)
                        << 120;
                } else {
                    unimplemented!("bad operand size");
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Paddusw | Mnemonic::Paddw => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value0");
                        return false;
                    }
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error getting value1");
                        return false;
                    }
                };
                let sz = self.get_operand_sz(&ins, 0);
                let mut result: u128;

                if sz == 64 {
                    result = ((value0 & 0xffff) as u16 + (value1 & 0xffff) as u16) as u128;
                    result |= ((((value0 & 0xffff0000) >> 16) as u16
                        + ((value1 & 0xffff0000) >> 16) as u16)
                        as u128)
                        << 16;
                    result |= ((((value0 & 0xffff00000000) >> 32) as u16
                        + ((value1 & 0xffff00000000) >> 32) as u16)
                        as u128)
                        << 32;
                    result |= ((((value0 & 0xffff000000000000) >> 48) as u16
                        + ((value1 & 0xffff000000000000) >> 48) as u16)
                        as u128)
                        << 48;
                } else if sz == 128 {
                    result = ((value0 & 0xffff) as u16 + (value1 & 0xffff) as u16) as u128;
                    result |= ((((value0 & 0xffff0000) >> 16) as u16
                        + ((value1 & 0xffff0000) >> 16) as u16)
                        as u128)
                        << 16;
                    result |= ((((value0 & 0xffff00000000) >> 32) as u16
                        + ((value1 & 0xffff00000000) >> 32) as u16)
                        as u128)
                        << 32;
                    result |= ((((value0 & 0xffff000000000000) >> 48) as u16
                        + ((value1 & 0xffff000000000000) >> 48) as u16)
                        as u128)
                        << 48;

                    result |= ((((value0 & 0xffff_0000000000000000) >> 64) as u16
                        + ((value1 & 0xffff_0000000000000000) >> 64) as u16)
                        as u128)
                        << 64;
                    result |= ((((value0 & 0xffff0000_0000000000000000) >> 80) as u16
                        + ((value1 & 0xffff0000_0000000000000000) >> 80) as u16)
                        as u128)
                        << 80;
                    result |= ((((value0 & 0xffff00000000_0000000000000000) >> 96) as u16
                        + ((value1 & 0xffff00000000_0000000000000000) >> 96) as u16)
                        as u128)
                        << 96;
                    result |= ((((value0 & 0xffff0000000000_0000000000000000) >> 112) as u16
                        + ((value1 & 0xffff0000000000_0000000000000000) >> 112) as u16)
                        as u128)
                        << 112;
                } else {
                    unimplemented!("bad operand size");
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Pshufd => {
                self.show_instruction(&self.colors.green, &ins);

                let source = self
                    .get_operand_xmm_value_128(&ins, 1, true)
                    .expect("error getting source");
                let order = self
                    .get_operand_value(&ins, 2, true)
                    .expect("error getting order");

                let order1 = get_bit!(order, 0) | (get_bit!(order, 1) << 1);
                let order2 = get_bit!(order, 2) | (get_bit!(order, 3) << 1);
                let order3 = get_bit!(order, 4) | (get_bit!(order, 5) << 1);
                let order4 = get_bit!(order, 6) | (get_bit!(order, 7) << 1);

                let mut dest: u128 = (source >> (order1 * 32)) as u32 as u128;
                dest |= ((source >> (order2 * 32)) as u32 as u128) << 32;
                dest |= ((source >> (order3 * 32)) as u32 as u128) << 64;
                dest |= ((source >> (order4 * 32)) as u32 as u128) << 96;

                self.set_operand_xmm_value_128(&ins, 0, dest);
            }

            Mnemonic::Movups => {
                self.show_instruction(&self.colors.green, &ins);

                let source = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error reading memory xmm 1 source operand");
                        return false;
                    }
                };

                self.set_operand_xmm_value_128(&ins, 0, source);
            }

            Mnemonic::Movdqu => {
                self.show_instruction(&self.colors.green, &ins);

                let source = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error reading memory xmm 1 source operand");
                        return false;
                    }
                };

                self.set_operand_xmm_value_128(&ins, 0, source);
            }

            // ymmX registers
            Mnemonic::Vzeroupper => {
                self.show_instruction(&self.colors.green, &ins);

                let mask_lower = regs64::U256::from(0xffffffffffffffffu64);
                let mask = mask_lower | (mask_lower << 64);

                self.regs.ymm0 &= mask;
                self.regs.ymm1 &= mask;
                self.regs.ymm2 &= mask;
                self.regs.ymm3 &= mask;
                self.regs.ymm4 &= mask;
                self.regs.ymm5 &= mask;
                self.regs.ymm6 &= mask;
                self.regs.ymm7 &= mask;
                self.regs.ymm8 &= mask;
                self.regs.ymm9 &= mask;
                self.regs.ymm10 &= mask;
                self.regs.ymm11 &= mask;
                self.regs.ymm12 &= mask;
                self.regs.ymm13 &= mask;
                self.regs.ymm14 &= mask;
                self.regs.ymm15 &= mask;
            }

            Mnemonic::Vmovdqu => {
                self.show_instruction(&self.colors.green, &ins);

                let sz0 = self.get_operand_sz(ins, 0);
                let sz1 = self.get_operand_sz(ins, 1);
                let sz_max = sz0.max(sz1);

                match sz_max {
                    128 => {
                        let source = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        self.set_operand_xmm_value_128(&ins, 0, source);
                    }
                    256 => {
                        let source = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        self.set_operand_ymm_value_256(&ins, 0, source);
                    }
                    _ => {
                        unimplemented!(
                            "unimplemented operand size {}",
                            self.get_operand_sz(ins, 1)
                        );
                    }
                }
            }

            Mnemonic::Vmovdqa => {
                //TODO: exception if memory address is unaligned to 16,32,64
                self.show_instruction(&self.colors.green, &ins);

                let sz0 = self.get_operand_sz(ins, 0);
                let sz1 = self.get_operand_sz(ins, 1);
                let sz_max = sz0.max(sz1);

                match sz_max {
                    128 => {
                        let source = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        self.set_operand_xmm_value_128(&ins, 0, source);
                    }
                    256 => {
                        let source = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        self.set_operand_ymm_value_256(&ins, 0, source);
                    }
                    _ => unimplemented!("unimplemented operand size"),
                }
            }

            Mnemonic::Movaps | Mnemonic::Movapd => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);

                let source = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error reading memory xmm 1 source operand");
                        return false;
                    }
                };

                self.set_operand_xmm_value_128(&ins, 0, source);
            }

            Mnemonic::Vmovd => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 2);
                assert!(self.get_operand_sz(&ins, 1) == 32);

                let value = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error reading second operand");
                        return false;
                    }
                };

                match self.get_operand_sz(&ins, 0) {
                    128 => {
                        self.set_operand_xmm_value_128(&ins, 0, value as u128);
                    }
                    256 => {
                        let result = regs64::U256::from(value);
                        self.set_operand_ymm_value_256(&ins, 0, result);
                    }
                    _ => unimplemented!(""),
                }
            }

            Mnemonic::Vmovq => {
                self.show_instruction(&self.colors.green, &ins);

                assert!(ins.op_count() == 2);
                assert!(self.get_operand_sz(&ins, 1) == 64);

                let value = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("error reading second operand");
                        return false;
                    }
                };

                match self.get_operand_sz(&ins, 0) {
                    128 => {
                        self.set_operand_xmm_value_128(&ins, 0, value as u128);
                    }
                    256 => {
                        let result = regs64::U256::from(value);
                        self.set_operand_ymm_value_256(&ins, 0, result);
                    }
                    _ => unimplemented!(""),
                }
            }

            Mnemonic::Vpbroadcastb => {
                self.show_instruction(&self.colors.green, &ins);

                let byte: u8;

                match self.get_operand_sz(&ins, 1) {
                    128 => {
                        let source = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        byte = (source & 0xff) as u8;
                    }

                    256 => {
                        let source = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        byte = (source & regs64::U256::from(0xFF)).low_u64() as u8;
                    }
                    _ => unreachable!(""),
                }

                match self.get_operand_sz(&ins, 0) {
                    128 => {
                        let mut result: u128 = 0;
                        for _ in 0..16 {
                            result <<= 8;
                            result |= byte as u128;
                        }
                        self.set_operand_xmm_value_128(&ins, 0, result);
                    }
                    256 => {
                        let mut result = regs64::U256::zero();
                        for _ in 0..32 {
                            result = result << 8;
                            result = result | regs64::U256::from(byte);
                        }
                        self.set_operand_ymm_value_256(&ins, 0, result);
                    }
                    _ => unreachable!(""),
                }
            }

            Mnemonic::Vpor => {
                self.show_instruction(&self.colors.green, &ins);

                match self.get_operand_sz(&ins, 1) {
                    128 => {
                        let source1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_xmm_value_128(&ins, 2, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 2 source operand");
                                return false;
                            }
                        };

                        self.set_operand_xmm_value_128(&ins, 0, source1 | source2);
                    }
                    256 => {
                        let source1 = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_ymm_value_256(&ins, 2, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 2 source operand");
                                return false;
                            }
                        };

                        self.set_operand_ymm_value_256(&ins, 0, source1 | source2);
                    }
                    _ => unreachable!(""),
                }
            }

            Mnemonic::Vpxor => {
                self.show_instruction(&self.colors.green, &ins);

                match self.get_operand_sz(&ins, 0) {
                    128 => {
                        let source1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_xmm_value_128(&ins, 2, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 2 source operand");
                                return false;
                            }
                        };

                        self.set_operand_xmm_value_128(&ins, 0, source1 ^ source2);
                    }
                    256 => {
                        let source1 = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_ymm_value_256(&ins, 2, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 2 source operand");
                                return false;
                            }
                        };

                        self.set_operand_ymm_value_256(&ins, 0, source1 ^ source2);
                    }
                    _ => unreachable!(""),
                }
            }

            Mnemonic::Pcmpeqb => {
                self.show_instruction(&self.colors.green, &ins);

                match self.get_operand_sz(ins, 0) {
                    128 => {
                        let source1 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 2 source operand");
                                return false;
                            }
                        };

                        let a_bytes = source1.to_le_bytes();
                        let b_bytes = source2.to_le_bytes();

                        let mut result = [0u8; 16];

                        for i in 0..16 {
                            if a_bytes[i] == b_bytes[i] {
                                result[i] = 0xFF;
                            } else {
                                result[i] = 0;
                            }
                        }

                        let result = u128::from_le_bytes(result);

                        self.set_operand_xmm_value_128(&ins, 0, result);
                    }
                    256 => {
                        let source1 = match self.get_operand_ymm_value_256(&ins, 0, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 2 source operand");
                                return false;
                            }
                        };

                        let mut bytes1: Vec<u8> = vec![0; 32];
                        source1.to_little_endian(&mut bytes1);
                        let mut bytes2: Vec<u8> = vec![0; 32];
                        source2.to_little_endian(&mut bytes2);

                        let mut result = [0u8; 32];

                        for i in 0..32 {
                            if bytes1[i] == bytes2[i] {
                                result[i] = 0xFF;
                            } else {
                                result[i] = 0;
                            }
                        }

                        let result256: regs64::U256 = regs64::U256::from_little_endian(&result);

                        self.set_operand_ymm_value_256(&ins, 0, result256);
                    }
                    _ => unreachable!(""),
                }
            }

            Mnemonic::Psubsb => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let mut result = 0u128;

                for i in 0..16 {
                    let mask = 0xFFu128;
                    let shift = i * 8;
                    let byte0 = ((value0 >> shift) & mask) as i8;
                    let byte1 = ((value1 >> shift) & mask) as i8;
                    let diff = byte0.saturating_sub(byte1);

                    result |= (diff as u128 & mask) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Fcomp => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = self.get_operand_value(&ins, 0, false).unwrap_or(0) as usize;
                let value2 = self.get_operand_value(&ins, 1, false).unwrap_or(2) as usize;

                let sti = self.fpu.get_st(value0);
                let stj = self.fpu.get_st(value2);

                self.fpu.f_c0 = sti < stj;
                self.fpu.f_c2 = sti.is_nan() || stj.is_nan();
                self.fpu.f_c3 = sti == stj;

                self.fpu.pop();
            }

            Mnemonic::Psrlq => {
                self.show_instruction(&self.colors.green, &ins);

                let destination = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let shift_amount = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let result = destination.wrapping_shr(shift_amount as u32);

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Psubsw => {
                self.show_instruction(&self.colors.green, &ins);

                // Obtener los valores de los registros XMM (128 bits cada uno)
                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0); // xmm6
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0); // xmm5
                let mut result = 0u128;

                for i in 0..8 {
                    let mask = 0xFFFFu128;
                    let shift = i * 16;
                    let word0 = ((value0 >> shift) & mask) as i16;
                    let word1 = ((value1 >> shift) & mask) as i16;
                    let diff = word0.saturating_sub(word1);

                    result |= (diff as u128 & mask) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Fsincos => {
                self.show_instruction(&self.colors.green, &ins);

                let st0 = self.fpu.get_st(0);
                let sin_value = st0.sin();
                let cos_value = st0.cos();

                self.fpu.set_st(0, sin_value);
                self.fpu.push(cos_value);
            }

            Mnemonic::Packuswb => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let mut result = 0u128;

                for i in 0..8 {
                    let mask = 0xFFFFu128;
                    let shift = i * 16;
                    let word0 = ((value0 >> shift) & mask) as i16;
                    let word1 = ((value1 >> shift) & mask) as i16;
                    let byte0 = if word0 > 255 {
                        255
                    } else if word0 < 0 {
                        0
                    } else {
                        word0 as u8
                    };
                    let byte1 = if word1 > 255 {
                        255
                    } else if word1 < 0 {
                        0
                    } else {
                        word1 as u8
                    };

                    result |= (byte0 as u128) << (i * 8);
                    result |= (byte1 as u128) << ((i + 8) * 8);
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Pandn => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0); // xmm1
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0); // xmm5
                let result = (!value0) & value1;

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Psrld => {
                self.show_instruction(&self.colors.green, &ins);

                let value = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let shift_amount =
                    self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0) as u32;
                let mut result = 0u128;

                for i in 0..4 {
                    let mask = 0xFFFFFFFFu128;
                    let shift = i * 32;
                    let dword = ((value >> shift) & mask) as u32;
                    let shifted = dword.wrapping_shr(shift_amount);

                    result |= (shifted as u128 & mask) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Punpckhwd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);

                let mut result = 0u128;

                for i in 0..4 {
                    let mask = 0xFFFFu128;
                    let shift = i * 16;

                    let word0 = ((value0 >> (shift + 48)) & mask) as u16;
                    let word1 = ((value1 >> (shift + 48)) & mask) as u16;

                    result |= (word0 as u128) << (i * 32);
                    result |= (word1 as u128) << (i * 32 + 16);
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Psraw => {
                self.show_instruction(&self.colors.green, &ins);

                let value1 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value6 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let mut result = 0u128;
                let shift_amount = (value6 & 0xFF) as u32;

                for i in 0..8 {
                    let mask = 0xFFFFu128;
                    let shift = i * 16;

                    let word = ((value1 >> shift) & mask) as i16;
                    let shifted_word = (word as i32 >> shift_amount) as i16;

                    result |= (shifted_word as u128) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Frndint => {
                self.show_instruction(&self.colors.green, &ins);

                let value = self.fpu.get_st(0);
                let rounded_value = value.round();

                self.fpu.set_st(0, rounded_value);
            }

            Mnemonic::Psrlw => {
                self.show_instruction(&self.colors.green, &ins);

                if self.get_operand_sz(&ins, 1) < 128 {
                    let value = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);

                    let shift_amount = match self.get_operand_value(&ins, 1, false) {
                        Some(v) => (v & 0xFF) as u32,
                        None => 0,
                    };

                    let mut result = 0u128;

                    for i in 0..8 {
                        let mask = 0xFFFFu128;
                        let shift = i * 16;
                        let word = ((value >> shift) & mask) as u16;
                        let shifted_word = (word as u32 >> shift_amount) as u16;

                        result |= (shifted_word as u128) << shift;
                    }

                    self.set_operand_xmm_value_128(&ins, 0, result);
                } else {
                    let value = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);

                    let shift_amount = match self.get_operand_xmm_value_128(&ins, 1, false) {
                        Some(v) => (v & 0xFF) as u32,
                        None => 0,
                    };

                    let mut result = 0u128;

                    for i in 0..8 {
                        let mask = 0xFFFFu128;
                        let shift = i * 16;
                        let word = ((value >> shift) & mask) as u16;
                        let shifted_word = (word as u32 >> shift_amount) as u16;

                        result |= (shifted_word as u128) << shift;
                    }

                    self.set_operand_xmm_value_128(&ins, 0, result);
                }
            }

            Mnemonic::Paddd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);

                let mut result = 0u128;

                for i in 0..4 {
                    let mask = 0xFFFFFFFFu128;
                    let shift = i * 32;
                    let word0 = ((value0 >> shift) & mask) as u32;
                    let word1 = ((value1 >> shift) & mask) as u32;
                    let sum = word0.wrapping_add(word1);

                    result |= (sum as u128) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Fscale => {
                self.show_instruction(&self.colors.green, &ins);

                let st0 = self.fpu.get_st(0);
                let st1 = self.fpu.get_st(1);

                let scale_factor = 2.0f64.powf(st1.trunc());
                let result = st0 * scale_factor;

                self.fpu.set_st(0, result);
            }

            Mnemonic::Vpcmpeqb => {
                self.show_instruction(&self.colors.green, &ins);

                match self.get_operand_sz(ins, 0) {
                    128 => {
                        let source1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_xmm_value_128(&ins, 2, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 2 source operand");
                                return false;
                            }
                        };

                        let a_bytes = source1.to_le_bytes();
                        let b_bytes = source2.to_le_bytes();

                        let mut result = [0u8; 16];

                        for i in 0..16 {
                            if a_bytes[i] == b_bytes[i] {
                                result[i] = 0xFF;
                            } else {
                                result[i] = 0;
                            }
                        }

                        let result = u128::from_le_bytes(result);

                        self.set_operand_xmm_value_128(&ins, 0, result);
                    }
                    256 => {
                        let source1 = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_ymm_value_256(&ins, 2, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 2 source operand");
                                return false;
                            }
                        };

                        let mut bytes1: Vec<u8> = vec![0; 32];
                        source1.to_little_endian(&mut bytes1);
                        let mut bytes2: Vec<u8> = vec![0; 32];
                        source2.to_little_endian(&mut bytes2);

                        let mut result = [0u8; 32];

                        for i in 0..32 {
                            if bytes1[i] == bytes2[i] {
                                result[i] = 0xFF;
                            } else {
                                result[i] = 0;
                            }
                        }

                        let result256: regs64::U256 = regs64::U256::from_little_endian(&result);

                        self.set_operand_ymm_value_256(&ins, 0, result256);
                    }
                    _ => unreachable!(""),
                }
            }

            Mnemonic::Pmullw => {
                self.show_instruction(&self.colors.green, &ins);

                let source0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let source1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let mut result = 0u128;

                for i in 0..8 {
                    let mask = 0xFFFFu128;
                    let shift = i * 16;
                    let word0 = ((source0 >> shift) & mask) as u16;
                    let word1 = ((source1 >> shift) & mask) as u16;
                    let product = word0.wrapping_mul(word1) as u128;
                    result |= (product & mask) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Pmulhw => {
                self.show_instruction(&self.colors.green, &ins);

                let source0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let source1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);
                let mut result = 0u128;

                for i in 0..8 {
                    let mask = 0xFFFFu128;
                    let shift = i * 16;

                    let word0 = ((source0 >> shift) & mask) as i16;
                    let word1 = ((source1 >> shift) & mask) as i16;
                    let product = (word0 as i32) * (word1 as i32);
                    let high_word = ((product >> 16) & 0xFFFF) as u128;
                    result |= high_word << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Pmovmskb => {
                self.show_instruction(&self.colors.green, &ins);

                match self.get_operand_sz(ins, 1) {
                    128 => {
                        let source1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        let mut result: u16 = 0;

                        for i in 0..16 {
                            let byte = ((source1 >> (i * 8)) & 0xff) as u16;
                            let msb = (byte & 0x80) >> 7;
                            result |= msb << i;
                        }

                        self.set_operand_value(&ins, 0, result as u64);
                    }
                    256 => {
                        let source1 = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        let mut result: u32 = 0;
                        let mut input_bytes = [0u8; 32];
                        source1.to_little_endian(&mut input_bytes);

                        for i in 0..32 {
                            let byte = input_bytes[i];
                            let msb = (byte & 0x80) >> 7;
                            result |= (msb as u32) << i;
                        }

                        self.set_operand_value(&ins, 0, result as u64);
                    }
                    _ => unreachable!(""),
                }
            }

            Mnemonic::Vpmovmskb => {
                self.show_instruction(&self.colors.green, &ins);

                match self.get_operand_sz(ins, 1) {
                    128 => {
                        let source1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        let mut result: u16 = 0;

                        for i in 0..16 {
                            let byte = ((source1 >> (i * 8)) & 0xff) as u16;
                            let msb = (byte & 0x80) >> 7;
                            result |= msb << i;
                        }

                        self.set_operand_value(&ins, 0, result as u64);
                    }
                    256 => {
                        let source1 = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        let mut result: u32 = 0;
                        let mut input_bytes = [0u8; 32];
                        source1.to_little_endian(&mut input_bytes);

                        for i in 0..32 {
                            let byte = input_bytes[i];
                            let msb = (byte & 0x80) >> 7;
                            result |= (msb as u32) << i;
                        }

                        self.set_operand_value(&ins, 0, result as u64);
                    }
                    _ => unreachable!(""),
                }
            }

            Mnemonic::Vpminub => {
                self.show_instruction(&self.colors.green, &ins);

                match self.get_operand_sz(ins, 0) {
                    128 => {
                        let source1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_xmm_value_128(&ins, 2, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 2 source operand");
                                return false;
                            }
                        };

                        let mut result: u128 = 0;
                        for i in 0..16 {
                            let byte1 = (source1 >> (8 * i)) & 0xFF;
                            let byte2 = (source2 >> (8 * i)) & 0xFF;
                            let min_byte = byte1.min(byte2);
                            result |= min_byte << (8 * i);
                        }

                        self.set_operand_xmm_value_128(&ins, 0, result);
                    }
                    256 => {
                        let source1 = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_ymm_value_256(&ins, 2, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 2 source operand");
                                return false;
                            }
                        };

                        let mut bytes1: Vec<u8> = vec![0; 32];
                        source1.to_little_endian(&mut bytes1);
                        let mut bytes2: Vec<u8> = vec![0; 32];
                        source2.to_little_endian(&mut bytes2);

                        let mut result = [0u8; 32];

                        for i in 0..32 {
                            result[i] = bytes1[i].min(bytes2[i]);
                        }

                        let result256: regs64::U256 = regs64::U256::from_little_endian(&result);

                        self.set_operand_ymm_value_256(&ins, 0, result256);
                    }
                    _ => unreachable!(""),
                }
            }

            Mnemonic::Fdecstp => {
                self.show_instruction(&self.colors.green, &ins);
                self.fpu.dec_top();
            }

            Mnemonic::Ftst => {
                self.show_instruction(&self.colors.green, &ins);

                let st0 = self.fpu.get_st(0);
                self.fpu.f_c0 = st0 < 0.0;
                self.fpu.f_c2 = st0.is_nan();
                self.fpu.f_c3 = st0 == 0.0;
            }

            Mnemonic::Emms => {
                self.show_instruction(&self.colors.green, &ins);
            }

            Mnemonic::Fxam => {
                self.show_instruction(&self.colors.green, &ins);

                let st0: f64 = self.fpu.get_st(0);

                if st0 < 0f64 {
                    self.fpu.f_c0 = true;
                } else {
                    self.fpu.f_c0 = false;
                }

                self.fpu.f_c1 = false;

                if st0.is_nan() {
                    self.fpu.f_c2 = true;
                    self.fpu.f_c3 = true;
                } else {
                    self.fpu.f_c2 = false;
                    self.fpu.f_c3 = false;
                }
            }

            Mnemonic::Pcmpgtw => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);
                assert!(self.get_operand_sz(&ins, 0) == 128);
                assert!(self.get_operand_sz(&ins, 1) == 128);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let mut result = 0u128;

                for i in 0..8 {
                    let shift = i * 16;
                    let word0 = (value0 >> shift) & 0xFFFF;
                    let word1 = (value1 >> shift) & 0xFFFF;

                    let cmp_result = if word0 > word1 {
                        0xFFFFu128
                    } else {
                        0x0000u128
                    };

                    result |= cmp_result << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Pcmpgtb => {
                self.show_instruction(&self.colors.green, &ins);
                assert!(ins.op_count() == 2);
                assert!(self.get_operand_sz(&ins, 0) == 128);
                assert!(self.get_operand_sz(&ins, 1) == 128);

                let value0 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };
                let value1 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let mut result = 0u128;

                for i in 0..16 {
                    let shift = i * 8;
                    let byte0 = (value0 >> shift) & 0xFF;
                    let byte1 = (value1 >> shift) & 0xFF;

                    let cmp_result = if byte0 > byte1 { 0xFFu128 } else { 0x00u128 };

                    result |= cmp_result << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Faddp => {
                self.show_instruction(&self.colors.green, &ins);

                let st0 = self.fpu.pop();
                let st1 = self.fpu.pop();

                self.fpu.push(st0 + st1);
            }

            Mnemonic::Pcmpeqw => {
                self.show_instruction(&self.colors.green, &ins);

                match self.get_operand_sz(ins, 0) {
                    128 => {
                        let source1 = match self.get_operand_xmm_value_128(&ins, 0, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_xmm_value_128(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory xmm 2 source operand");
                                return false;
                            }
                        };

                        let a_words = source1.to_le_bytes();
                        let b_words = source2.to_le_bytes();

                        let mut result = [0u8; 16];

                        for i in 0..8 {
                            let word_a = u16::from_le_bytes([a_words[2 * i], a_words[2 * i + 1]]);
                            let word_b = u16::from_le_bytes([b_words[2 * i], b_words[2 * i + 1]]);
                            let cmp_result: u16 = if word_a == word_b { 0xFFFF } else { 0x0000 };
                            let [low, high] = cmp_result.to_le_bytes();
                            result[2 * i] = low;
                            result[2 * i + 1] = high;
                        }
                        let result = u128::from_le_bytes(result);
                        self.set_operand_xmm_value_128(&ins, 0, result);
                    }
                    256 => {
                        let source1 = match self.get_operand_ymm_value_256(&ins, 0, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 1 source operand");
                                return false;
                            }
                        };

                        let source2 = match self.get_operand_ymm_value_256(&ins, 1, true) {
                            Some(v) => v,
                            None => {
                                println!("error reading memory ymm 2 source operand");
                                return false;
                            }
                        };

                        let mut bytes1: Vec<u8> = vec![0; 32];
                        source1.to_little_endian(&mut bytes1);
                        let mut bytes2: Vec<u8> = vec![0; 32];
                        source2.to_little_endian(&mut bytes2);
                        let mut result = [0u8; 32];

                        for i in 0..16 {
                            let word1 = u16::from_le_bytes([bytes1[2 * i], bytes1[2 * i + 1]]);
                            let word2 = u16::from_le_bytes([bytes2[2 * i], bytes2[2 * i + 1]]);
                            let cmp_result = if word1 == word2 { 0xFFFFu16 } else { 0x0000u16 };
                            let [low, high] = cmp_result.to_le_bytes();

                            result[2 * i] = low;
                            result[2 * i + 1] = high;
                        }

                        let result256: regs64::U256 = regs64::U256::from_little_endian(&result);
                        self.set_operand_ymm_value_256(&ins, 0, result256);
                    }
                    _ => unreachable!(""),
                }
            }

            Mnemonic::Fnclex => {
                self.show_instruction(&self.colors.green, &ins);
                self.fpu.stat &= !(0b10000011_11111111);
            }

            Mnemonic::Fcom => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);

                let value1 = match self.get_operand_value(&ins, 1, false) {
                    Some(v1) => v1,
                    None => 0,
                };

                let st4 = self.fpu.get_st(value1 as usize);

                if st0.is_nan() || st4.is_nan() {
                    self.fpu.f_c0 = false;
                    self.fpu.f_c2 = true;
                    self.fpu.f_c3 = false;
                } else {
                    self.fpu.f_c0 = st0 < st4;
                    self.fpu.f_c2 = false;
                    self.fpu.f_c3 = st0 == st4;
                }
            }

            Mnemonic::Fmul => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);

                let value1 = match self.get_operand_value(&ins, 1, false) {
                    Some(v1) => v1,
                    None => 0,
                };

                let stn = self.fpu.get_st(value1 as usize);
                self.fpu.set_st(0, st0 * stn);
            }

            Mnemonic::Fabs => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);
                self.fpu.set_st(0, st0.abs());
            }

            Mnemonic::Fsin => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);
                self.fpu.set_st(0, st0.sin());
            }

            Mnemonic::Fcos => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);
                self.fpu.set_st(0, st0.cos());
            }

            Mnemonic::Fdiv => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);

                let value1 = match self.get_operand_value(&ins, 1, false) {
                    Some(v1) => v1,
                    None => 0,
                };

                let stn = self.fpu.get_st(value1 as usize);
                self.fpu.set_st(0, st0 / stn);
            }

            Mnemonic::Fdivr => {
                self.show_instruction(&self.colors.green, &ins);
                let st0 = self.fpu.get_st(0);
                let value1 = self.get_operand_value(&ins, 1, false).unwrap_or(0);
                let stn = self.fpu.get_st(value1 as usize);
                self.fpu.set_st(0, stn / st0);
            }

            Mnemonic::Fdivrp => {
                self.show_instruction(&self.colors.green, &ins);
                let value0 = self.get_operand_value(&ins, 0, false).unwrap_or(0) as usize;
                let value1 = self.get_operand_value(&ins, 1, false).unwrap_or(0) as usize;
                let st0 = self.fpu.get_st(value0);
                let st7 = self.fpu.get_st(value1);

                let result = st7 / st0;

                self.fpu.set_st(value1, result);
                self.fpu.pop();
            }

            Mnemonic::Fpatan => {
                self.show_instruction(&self.colors.green, &ins);

                let st0 = self.fpu.get_st(0);
                let st1 = self.fpu.get_st(1);
                let result = (st1 / st0).atan();
                self.fpu.set_st(1, result);
                self.fpu.pop();
            }

            Mnemonic::Fprem => {
                self.show_instruction(&self.colors.green, &ins);

                let st0 = self.fpu.get_st(0);
                let st1 = self.fpu.get_st(1);

                let quotient = (st0 / st1).floor();
                let result = st0 - quotient * st1;

                self.fpu.set_st(0, result);
            }

            Mnemonic::Fprem1 => {
                self.show_instruction(&self.colors.green, &ins);

                let st0 = self.fpu.get_st(0);
                let st1 = self.fpu.get_st(1);

                let quotient = (st0 / st1).round();
                let remainder = st0 - quotient * st1;

                self.fpu.set_st(0, remainder);
            }

            Mnemonic::Pcmpgtd => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let value1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);

                let mut result = 0u128;

                for i in 0..4 {
                    let shift = i * 32;
                    let word0 = ((value0 >> shift) & 0xFFFFFFFFu128) as u32;
                    let word1 = ((value1 >> shift) & 0xFFFFFFFFu128) as u32;
                    let comparison_result = if word0 > word1 {
                        0xFFFFFFFFu32
                    } else {
                        0x00000000u32
                    };

                    result |= (comparison_result as u128) << shift;
                }

                self.set_operand_xmm_value_128(&ins, 0, result);
            }

            Mnemonic::Pmaddwd => {
                self.show_instruction(&self.colors.green, &ins);

                let src0 = self.get_operand_xmm_value_128(&ins, 0, true).unwrap_or(0);
                let src1 = self.get_operand_xmm_value_128(&ins, 1, true).unwrap_or(0);

                let mut result = [0i32; 2];

                for i in 0..4 {
                    let shift = i * 16;
                    let a = ((src0 >> shift) & 0xFFFF) as i16 as i32;
                    let b = ((src1 >> shift) & 0xFFFF) as i16 as i32;

                    let product = a * b;

                    if i < 2 {
                        result[0] += product;
                    } else {
                        result[1] += product;
                    }
                }

                let final_result = ((result[1] as u64) << 32) | (result[0] as u64);

                self.set_operand_xmm_value_128(&ins, 0, final_result as u128);
            }

            // end SSE
            Mnemonic::Tzcnt => {
                self.show_instruction(&self.colors.green, &ins);

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let sz = self.get_operand_sz(&ins, 0) as u64;
                let mut temp: u64 = 0;
                let mut dest: u64 = 0;

                while temp < sz && get_bit!(value1, temp) == 0 {
                    temp += 1;
                    dest += 1;
                }

                self.flags.f_cf = dest == sz;
                self.flags.f_zf = dest == 0;

                self.set_operand_value(&ins, 1, dest);
            }

            Mnemonic::Xgetbv => {
                self.show_instruction(&self.colors.green, &ins);

                match self.regs.get_ecx() {
                    0 => {
                        self.regs.set_edx(0);
                        self.regs.set_eax(0x1f); //7
                    }
                    _ => {
                        self.regs.set_edx(0);
                        self.regs.set_eax(7);
                    }
                }
            }

            Mnemonic::Arpl => {
                self.show_instruction(&self.colors.green, &ins);

                let value0 = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let value1 = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                self.flags.f_zf = value1 < value0;

                self.set_operand_value(&ins, 0, value0);
            }

            Mnemonic::Pushf => {
                self.show_instruction(&self.colors.blue, &ins);

                let val: u16 = (self.flags.dump() & 0xffff) as u16;

                self.regs.rsp -= 2;

                if !self.maps.write_word(self.regs.rsp, val) {
                    println!("/!\\ exception writing word at rsp 0x{:x}", self.regs.rsp);
                    self.exception();
                    return false;
                }
            }

            Mnemonic::Pushfd => {
                self.show_instruction(&self.colors.blue, &ins);

                // 32bits only instruction
                let flags = self.flags.dump();
                if !self.stack_push32(flags) {
                    return false;
                }
            }

            Mnemonic::Pushfq => {
                self.show_instruction(&self.colors.blue, &ins);
                self.flags.f_tf = false;
                if !self.stack_push64(self.flags.dump() as u64) {
                    return false;
                }
            }

            Mnemonic::Bound => {
                self.show_instruction(&self.colors.red, &ins);

                let array_index = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => {
                        println!("cannot read first opreand of bound");
                        return false;
                    }
                };
                let lower_upper_bound = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => {
                        println!("cannot read second opreand of bound");
                        return false;
                    }
                };

                println!(
                    "bound idx:{} lower_upper:{}",
                    array_index, lower_upper_bound
                );
                println!("Bound unimplemented");
                return false;
                // https://www.felixcloutier.com/x86/bound
            }

            Mnemonic::Lahf => {
                self.show_instruction(&self.colors.red, &ins);

                //println!("\tlahf: flags = {:?}", self.flags);

                let mut result: u8 = 0;
                set_bit!(result, 0, self.flags.f_cf as u8);
                set_bit!(result, 1, true as u8);
                set_bit!(result, 2, self.flags.f_pf as u8);
                set_bit!(result, 3, false as u8);
                set_bit!(result, 4, self.flags.f_af as u8);
                set_bit!(result, 5, false as u8);
                set_bit!(result, 6, self.flags.f_zf as u8);
                set_bit!(result, 7, self.flags.f_sf as u8);
                self.regs.set_ah(result as u64);
            }

            Mnemonic::Salc => {
                self.show_instruction(&self.colors.red, &ins);

                if self.flags.f_cf {
                    self.regs.set_al(1);
                } else {
                    self.regs.set_al(0);
                }
            }

            Mnemonic::Prefetchw => {
                self.show_instruction(&self.colors.red, &ins);
            }

            Mnemonic::Pause => {
                self.show_instruction(&self.colors.red, &ins);
            }

            Mnemonic::Wait => {
                self.show_instruction(&self.colors.red, &ins);
            }

            Mnemonic::Mwait => {
                self.show_instruction(&self.colors.red, &ins);
            }

            Mnemonic::Endbr64 => {
                self.show_instruction(&self.colors.red, &ins);
            }

            Mnemonic::Endbr32 => {
                self.show_instruction(&self.colors.red, &ins);
            }

            Mnemonic::Enqcmd => {
                self.show_instruction(&self.colors.red, &ins);
            }

            Mnemonic::Enqcmds => {
                self.show_instruction(&self.colors.red, &ins);
            }

            Mnemonic::Enter => {
                self.show_instruction(&self.colors.red, &ins);

                let allocSZ = match self.get_operand_value(&ins, 0, true) {
                    Some(v) => v,
                    None => return false,
                };

                let nestingLvl = match self.get_operand_value(&ins, 1, true) {
                    Some(v) => v,
                    None => return false,
                };

                let frameTmp;

                if self.cfg.is_64bits {
                    self.stack_push64(self.regs.rbp);
                    frameTmp = self.regs.rsp;
                } else {
                    self.stack_push32(self.regs.get_ebp() as u32);
                    frameTmp = self.regs.get_esp();
                }

                if nestingLvl > 1 {
                    for i in 1..nestingLvl {
                        if self.cfg.is_64bits {
                            self.regs.rbp -= 8;
                            self.stack_push64(self.regs.rbp);
                        } else {
                            self.regs.set_ebp(self.regs.get_ebp() - 4);
                            self.stack_push32(self.regs.get_ebp() as u32);
                        }
                    }
                } else {
                    if self.cfg.is_64bits {
                        self.stack_push64(frameTmp);
                    } else {
                        self.stack_push32(frameTmp as u32);
                    }
                }

                if self.cfg.is_64bits {
                    self.regs.rbp = frameTmp;
                    self.regs.rsp -= allocSZ;
                } else {
                    self.regs.set_ebp(frameTmp);
                    self.regs.set_esp(self.regs.get_esp() - allocSZ);
                }
            }

            ////   Ring0  ////
            Mnemonic::Rdmsr => {
                self.show_instruction(&self.colors.red, &ins);

                match self.regs.rcx {
                    0x176 => {
                        self.regs.rdx = 0;
                        self.regs.rax = self.cfg.code_base_addr + 0x42;
                    }
                    _ => {
                        println!("/!\\ unimplemented rdmsr with value {}", self.regs.rcx);
                        return false;
                    }
                }
            }

            _ => {
                if self.cfg.verbose >= 2 || !self.cfg.skip_unimplemented {
                    if self.cfg.is_64bits {
                        println!(
                            "{}{} 0x{:x}: {}{}",
                            self.colors.red,
                            self.pos,
                            ins.ip(),
                            self.out,
                            self.colors.nc
                        );
                    } else {
                        println!(
                            "{}{} 0x{:x}: {}{}",
                            self.colors.red,
                            self.pos,
                            ins.ip32(),
                            self.out,
                            self.colors.nc
                        );
                    }
                }

                if !self.cfg.skip_unimplemented {
                    println!("unimplemented or invalid instruction. use --banzai (cfg.skip_unimplemented) mode to skip");
                    if self.cfg.console_enabled {
                        self.spawn_console();
                    }
                    return false;
                    //unimplemented!("unimplemented instruction");
                }
            }
        }

        return true; // result_ok
    }
}
