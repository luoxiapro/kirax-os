//! 寄存器虚拟机解释器（DEX 字节码执行引擎）。
//!
//! 支持（MVP，真实可执行）：
//! - 常量加载（const/4、const/16、const、const/high16、const-wide 系、const-string）、move 全系
//! - 整数算术/逻辑/移位（二进制、/2addr、/lit8、/lit16）、一元 neg/not、int-to-* 转换
//! - 分支（goto 全系、if-test、if-testz）、cmp-long
//! - 方法调用（static/direct/virtual/interface + /range，递归）、返回全系、move-result
//! - 对象句柄模型 + 实例字段（iget/iput）与静态字段（sget/sput）
//! - new-instance、monitor(no-op)；throw 终止并报错；Native 桩（Math.abs/max/min）
//!
//! 未实现（报明确错误）：数组、异常表（tries）、浮点运算、packed/sparse-switch、
//! 真实多态分派（virtual 按 method_idx 静态解析）、const-class。

use crate::dex::{code_insns, type_short, DexFile, Method};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpErr {
    UnsupportedOp(u8),
    RegOutOfBounds,
    NoSuchMethod,
    StackOverflow,
    BadInvoke,
    BadObject,
    Throw,
    BadCode,
    ArrayOpsUnsupported,
}

pub type R<T> = core::result::Result<T, InterpErr>;

const MAX_FRAMES: usize = 1024;

#[derive(Debug, Clone)]
pub struct Obj {
    pub class: String,
    /// 字段槽：key = field_idx
    pub fields: BTreeMap<u32, u32>,
}

#[derive(Debug, Clone)]
struct Frame {
    mid: u32,
    pc: usize,
    regs: Vec<u32>,
    pending_ret: Option<u64>,
}

enum Step {
    Next(usize),
    Ret(u64),
}

pub struct Interp<'a> {
    dex: &'a DexFile<'a>,
    frames: Vec<Frame>,
    objects: Vec<Obj>,
    static_fields: BTreeMap<u32, u32>,
    pub max_frames: usize,
}

impl<'a> Interp<'a> {
    pub fn new(dex: &'a DexFile<'a>) -> Self {
        Interp {
            dex,
            frames: Vec::new(),
            objects: Vec::new(),
            static_fields: BTreeMap::new(),
            max_frames: MAX_FRAMES,
        }
    }

    /// 入口：按 类描述符或短名/方法名/原型 调用静态方法。proto 如 "(I)I"。
    pub fn invoke_static(&mut self, class: &str, name: &str, proto: &str, args: &[u32]) -> R<u64> {
        for m in &self.dex.methods {
            let class_ok = m.class == class
                || (!class.starts_with('L') && m.class.starts_with('L') && type_short(&m.class) == class);
            if class_ok && m.name == name && m.shorty == proto {
                return self.invoke_method(m.idx, args, true);
            }
        }
        Err(InterpErr::NoSuchMethod)
    }

    pub fn invoke_method(&mut self, mid: u32, args: &[u32], entry: bool) -> R<u64> {
        if entry {
            self.frames.clear();
        }
        let m = self.dex.method(mid).ok_or(InterpErr::NoSuchMethod)?.clone();
        if self.frames.len() >= self.max_frames {
            return Err(InterpErr::StackOverflow);
        }
        let widths = param_widths(&m.params);
        let need: usize = widths.iter().map(|w| *w as usize).sum();
        if args.len() < need {
            return Err(InterpErr::BadInvoke);
        }
        if m.code_off < 0 {
            return self.native_call(&m, args, &widths);
        }
        let (insns, registers, _ins_size, _outs) =
            code_insns(self.dex.data, m.code_off as u32).ok_or(InterpErr::BadCode)?;
        let _ = insns;
        let mut regs = vec![0u32; registers as usize];
        let (mut src, mut dst) = (0usize, 0usize);
        for w in &widths {
            if *w == 2 {
                if src + 1 < args.len() && dst + 1 < regs.len() {
                    regs[dst] = args[src];
                    regs[dst + 1] = args[src + 1];
                }
                src += 2;
                dst += 2;
            } else {
                if src < args.len() && dst < regs.len() {
                    regs[dst] = args[src];
                }
                src += 1;
                dst += 1;
            }
        }
        self.frames.push(Frame { mid, pc: 0, regs, pending_ret: None });
        self.run()
    }

    /// 解释循环：执行栈顶帧直到它 return（嵌套调用在 step 内递归完成）
    fn run(&mut self) -> R<u64> {
        loop {
            let Some(top) = self.frames.last() else { break };
            let m = self.dex.method(top.mid).ok_or(InterpErr::NoSuchMethod)?.clone();
            let (insns, _r, _i, _o) =
                code_insns(self.dex.data, m.code_off as u32).ok_or(InterpErr::BadCode)?;
            let pc = top.pc;
            match self.step(&m, insns, pc)? {
                Step::Next(n) => {
                    self.frames.last_mut().unwrap().pc = n;
                }
                Step::Ret(v) => {
                    self.frames.pop();
                    return Ok(v);
                }
            }
        }
        Err(InterpErr::BadCode)
    }

    // ------------------------------------------------------------------
    // 指令执行（格式长度以 DEX 规范 code units 计）
    // ------------------------------------------------------------------

    fn step(&mut self, _m: &Method, insns: &[u8], pc: usize) -> R<Step> {
        let w = word(insns, pc)?;
        let op = (w & 0xff) as u8;
        let inst = (w >> 8) as u16;

        macro_rules! nxt {
            ($units:expr) => {
                Step::Next(pc + ($units as usize) * 2)
            };
        }

        Ok(match op {
            // ---------- 10x / 12x / 22x / 32x ----------
            0x00 => nxt!(1), // nop
            0x01..=0x09 => {
                // move 系
                let f16 = matches!(op, 0x02 | 0x05 | 0x08);
                let f32 = matches!(op, 0x03 | 0x06 | 0x09);
                let wide = matches!(op, 0x04 | 0x05 | 0x06);
                let (a, b) = if f32 {
                    (word(insns, pc + 2)? as usize, word(insns, pc + 4)? as usize)
                } else if f16 {
                    ((inst & 0xff) as usize, (inst >> 8) as usize)
                } else {
                    ((inst & 0x0f) as usize, (inst >> 4) as usize)
                };
                let v = self.reg(a)?;
                if wide {
                    let v2 = self.reg(a + 1)?;
                    self.set_reg(b, v)?;
                    self.set_reg(b + 1, v2)?;
                } else {
                    self.set_reg(b, v)?;
                }
                nxt!(if f32 { 3 } else if f16 { 2 } else { 1 })
            }
            0x0a..=0x0c => {
                // move-result / -wide / -object
                let a = (inst & 0x0f) as usize;
                let wide = op == 0x0b;
                let v = self.top().pending_ret.ok_or(InterpErr::BadCode)?;
                self.set_reg(a, (v & 0xffff_ffff) as u32)?;
                if wide {
                    self.set_reg(a + 1, (v >> 32) as u32)?;
                }
                nxt!(1)
            }
            0x0e => Step::Ret(0), // return-void
            0x0f => Step::Ret(self.reg((inst & 0x0f) as usize)? as u64),
            0x10 => {
                let a = (inst & 0x0f) as usize;
                let lo = self.reg(a)?;
                let hi = self.reg(a + 1)?;
                Step::Ret(((hi as u64) << 32) | lo as u64)
            }
            0x11 => Step::Ret(self.reg((inst & 0x0f) as usize)? as u64),

            // ---------- 常量 ----------
            0x12 => {
                let a = (inst & 0x0f) as usize;
                let b = ((inst >> 4) as i8) as i32;
                self.set_reg(a, b as u32)?;
                nxt!(1)
            }
            0x13 => {
                let a = (inst & 0xff) as usize;
                let b = word(insns, pc + 2)? as i16 as i32;
                self.set_reg(a, b as u32)?;
                nxt!(2)
            }
            0x14 => {
                let a = (inst & 0xff) as usize;
                let b = i32_at(insns, pc + 2)?;
                self.set_reg(a, b as u32)?;
                nxt!(3)
            }
            0x15 => {
                let a = (inst & 0xff) as usize;
                let b = (word(insns, pc + 2)? as i16 as i32) << 16;
                self.set_reg(a, b as u32)?;
                nxt!(2)
            }
            0x16 => {
                let a = (inst & 0xff) as usize;
                let b = word(insns, pc + 2)? as i16 as i64;
                self.set_wide(a, b)?;
                nxt!(2)
            }
            0x17 => {
                let a = (inst & 0xff) as usize;
                let b = i32_at(insns, pc + 2)? as i64;
                self.set_wide(a, b)?;
                nxt!(3)
            }
            0x18 => {
                let a = (inst & 0xff) as usize;
                let b = u64_at(insns, pc + 2)?;
                self.set_wide(a, b as i64)?;
                nxt!(5)
            }
            0x19 => {
                let a = (inst & 0xff) as usize;
                let b = ((word(insns, pc + 2)? as u64) << 32) as i64;
                self.set_wide(a, b)?;
                nxt!(2)
            }
            0x1a | 0x1b => {
                let a = (inst & 0xff) as usize;
                let sidx = if op == 0x1a {
                    word(insns, pc + 2)? as usize
                } else {
                    u32_at(insns, pc + 2)? as usize
                };
                let s = self.dex.strings.get(sidx).cloned().unwrap_or_default();
                let h = self.alloc_string(s);
                self.set_reg(a, h)?;
                nxt!(if op == 0x1a { 2 } else { 3 })
            }
            0x1d => {
                let a = (inst & 0xff) as usize;
                let t = word(insns, pc + 2)? as usize;
                let cls = self.dex.types.get(t).cloned().unwrap_or_default();
                let h = self.alloc_obj(cls);
                self.set_reg(a, h)?;
                nxt!(3)
            }
            0x22 => return Err(InterpErr::Throw),

            // ---------- goto ----------
            0x23 => {
                let off = ((inst as i8) as i32) * 2;
                Step::Next((pc as i32 + off) as usize)
            }
            0x24 => {
                let off = (word(insns, pc + 2)? as i16 as i32) * 2;
                Step::Next((pc as i32 + off) as usize)
            }
            0x25 => {
                let off = i32_at(insns, pc + 2)? * 2;
                Step::Next((pc as i32 + off) as usize)
            }

            // ---------- cmp（23x，3 units） ----------
            0x2c => {
                let a = (inst & 0xff) as usize;
                let w1 = word(insns, pc + 2)?;
                let b = (w1 & 0xff) as usize;
                let c = ((w1 >> 8) | (word(insns, pc + 4)? << 8)) as usize;
                let bv = self.wide(b)?;
                let cv = self.wide(c)?;
                self.set_reg(a, (bv.cmp(&cv) as i32) as u32)?;
                nxt!(3)
            }

            // ---------- if-test（22t，3 units） ----------
            0x2d..=0x32 => {
                let a = (inst & 0xff) as usize;
                let w1 = word(insns, pc + 2)?;
                let b = (w1 & 0xff) as usize;
                let off = (((w1 >> 8) | (word(insns, pc + 4)? << 8)) as i16 as i32) * 2;
                let va = self.reg(a)? as i32;
                let vb = self.reg(b)? as i32;
                let taken = match op {
                    0x2d => va == vb,
                    0x2e => va != vb,
                    0x2f => va < vb,
                    0x30 => va >= vb,
                    0x31 => va > vb,
                    _ => va <= vb,
                };
                Step::Next(if taken { (pc as i32 + off) as usize } else { pc + 6 })
            }
            // if-testz（21t，2 units）
            0x33..=0x38 => {
                let a = (inst & 0xff) as usize;
                let off = (word(insns, pc + 2)? as i16 as i32) * 2;
                let va = self.reg(a)? as i32;
                let taken = match op {
                    0x33 => va == 0,
                    0x34 => va != 0,
                    0x35 => va < 0,
                    0x36 => va >= 0,
                    0x37 => va > 0,
                    _ => va <= 0,
                };
                Step::Next(if taken { (pc as i32 + off) as usize } else { pc + 4 })
            }

            // ---------- 数组（未实现） ----------
            0x46..=0x4f => return Err(InterpErr::ArrayOpsUnsupported),

            // ---------- 实例字段 iget/iput（22c，3 units） ----------
            0x52..=0x5f => {
                let a = (inst & 0xff) as usize;
                let w1 = word(insns, pc + 2)?;
                let b = (w1 & 0xff) as usize;
                let fidx = ((w1 >> 8) | (word(insns, pc + 4)? << 8)) as u32;
                if op < 0x59 {
                    let obj = self.reg(b)?;
                    let v = self.instance_field(obj, fidx)?;
                    self.set_reg(a, v)?;
                } else {
                    let v = self.reg(a)?;
                    let obj = self.reg(b)?;
                    self.set_instance_field(obj, fidx, v)?;
                }
                nxt!(3)
            }
            // 静态字段 sget/sput（21c，2 units）
            0x60..=0x6d => {
                let a = (inst & 0xff) as usize;
                let fidx = word(insns, pc + 2)? as u32;
                if op < 0x67 {
                    let v = self.static_fields.get(&fidx).copied().unwrap_or(0);
                    self.set_reg(a, v)?;
                } else {
                    let v = self.reg(a)?;
                    self.static_fields.insert(fidx, v);
                }
                nxt!(2)
            }

            // ---------- 一元/转换（12x，1 unit） ----------
            0x7b..=0x90 => {
                let a = (inst & 0xff) as usize;
                let b = (inst >> 8) as usize;
                let v: u32 = match op {
                    0x7b => (self.reg(b)? as i32).wrapping_neg() as u32,
                    0x7c => !self.reg(b)?,
                    0x7d => self.wide(b)?.wrapping_neg() as u32,
                    0x7e => !(self.wide(b)?) as u32,
                    0x81 => self.wide(b)? as u32,
                    0x84 => (self.wide(b)? & 0xffff_ffff) as u32,
                    0x8e => (self.reg(b)? as u8) as u32,
                    0x8f => (self.reg(b)? as u16) as u32,
                    0x90 => (self.reg(b)? as u16 as i16) as u32,
                    _ => return Err(InterpErr::UnsupportedOp(op)),
                };
                self.set_reg(a, v)?;
                nxt!(1)
            }

            // ---------- 二进制 int（23x，3 units） ----------
            0x91..=0x9b => {
                let a = (inst & 0xff) as usize;
                let w1 = word(insns, pc + 2)?;
                let b = (w1 & 0xff) as usize;
                let c = ((w1 >> 8) | (word(insns, pc + 4)? << 8)) as usize;
                let x = self.reg(b)? as i32;
                let y = self.reg(c)? as i32;
                let v = binop_int(op, x, y)?;
                self.set_reg(a, v as u32)?;
                nxt!(3)
            }
            // 二进制 long（23x，3 units）
            0x9c..=0xa8 => {
                let a = (inst & 0xff) as usize;
                let w1 = word(insns, pc + 2)?;
                let b = (w1 & 0xff) as usize;
                let c = ((w1 >> 8) | (word(insns, pc + 4)? << 8)) as usize;
                let x = self.wide(b)? as i64;
                let y = self.wide(c)? as i64;
                let v = binop_long(op, x, y)?;
                self.set_wide(a, v)?;
                nxt!(3)
            }
            0xb0..=0xcf => return Err(InterpErr::UnsupportedOp(op)),

            // ---------- /2addr（12x，1 unit） ----------
            0xd0..=0xd7 => {
                let a = (inst & 0x0f) as usize;
                let b = (inst >> 4) as usize;
                let x = self.reg(a)? as i32;
                let y = self.reg(b)? as i32;
                let v = binop_int(op, x, y)?;
                self.set_reg(a, v as u32)?;
                nxt!(1)
            }

            // ---------- /lit16（22s，3 units） ----------
            0xd8..=0xdf => {
                let a = (inst & 0xff) as usize;
                let w1 = word(insns, pc + 2)?;
                let b = (w1 & 0xff) as usize;
                let lit = (((w1 >> 8) | (word(insns, pc + 4)? << 8)) as i16) as i32;
                let x = self.reg(b)? as i32;
                let v = binop_lit(op, x, lit)?;
                self.set_reg(a, v as u32)?;
                nxt!(3)
            }

            // ---------- /lit8（22b，2 units） ----------
            0xe0..=0xea => {
                let a = (inst & 0xff) as usize;
                let w1 = word(insns, pc + 2)?;
                let b = (w1 & 0xff) as usize;
                let lit = ((w1 >> 8) as i8) as i32;
                let x = self.reg(b)? as i32;
                let v = binop_lit(op, x, lit)?;
                self.set_reg(a, v as u32)?;
                nxt!(2)
            }

            // ---------- invoke（35c / 3rc，3 units） ----------
            0x6e..=0x72 | 0x74..=0x78 => {
                let range = op >= 0x74;
                let midx = word(insns, pc + 2)? as u32;
                let m2 = self.dex.method(midx).ok_or(InterpErr::NoSuchMethod)?.clone();
                let widths = param_widths(&m2.params);
                let mut arg_regs: Vec<u32> = Vec::new();
                if range {
                    let n = inst as usize;
                    let c = word(insns, pc + 4)? as usize;
                    let mut need = 0usize;
                    for w in &widths {
                        need += *w as usize;
                    }
                    if need > n {
                        return Err(InterpErr::BadInvoke);
                    }
                    let mut r = c;
                    for w in &widths {
                        arg_regs.push(self.reg(r)?);
                        if *w == 2 {
                            arg_regs.push(self.reg(r + 1)?);
                        }
                        r += *w as usize;
                    }
                } else {
                    let count = (inst & 0x0f) as usize;
                    // 35c：寄存器槽 C..G（半字节），G 在 inst 高 4 位
                    let slots = (word(insns, pc + 4)? as u32) | (((inst as u32) >> 4) << 16);
                    let mut need = 0usize;
                    for w in &widths {
                        need += *w as usize;
                    }
                    if need > count {
                        return Err(InterpErr::BadInvoke);
                    }
                    let mut slot = 0usize;
                    for w in &widths {
                        let r = ((slots >> (slot * 4)) & 0x0f) as usize;
                        arg_regs.push(self.reg(r)?);
                        if *w == 2 {
                            arg_regs.push(self.reg(r + 1)?);
                        }
                        slot += *w as usize;
                    }
                }
                let ret = self.invoke_method(midx, &arg_regs, false)?;
                self.top_mut().pending_ret = Some(ret);
                nxt!(3)
            }

            _ => return Err(InterpErr::UnsupportedOp(op)),
        })
    }

    // ------------------------------------------------------------------
    // 辅助
    // ------------------------------------------------------------------

    fn top(&self) -> &Frame {
        self.frames.last().expect("frame")
    }

    fn top_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("frame")
    }

    fn reg(&self, r: usize) -> R<u32> {
        self.top().regs.get(r).copied().ok_or(InterpErr::RegOutOfBounds)
    }

    fn set_reg(&mut self, r: usize, v: u32) -> R<()> {
        if r >= self.top().regs.len() {
            return Err(InterpErr::RegOutOfBounds);
        }
        self.top_mut().regs[r] = v;
        Ok(())
    }

    fn wide(&self, r: usize) -> R<u64> {
        let lo = self.reg(r)?;
        let hi = self.reg(r + 1)?;
        Ok(((hi as u64) << 32) | lo as u64)
    }

    fn set_wide(&mut self, r: usize, v: i64) -> R<()> {
        self.set_reg(r, v as u32)?;
        self.set_reg(r + 1, (v >> 32) as u32)
    }

    fn alloc_obj(&mut self, class: String) -> u32 {
        self.objects.push(Obj { class, fields: BTreeMap::new() });
        self.objects.len() as u32
    }

    fn alloc_string(&mut self, s: String) -> u32 {
        let mut f = BTreeMap::new();
        f.insert(0, str_hash(&s));
        self.objects.push(Obj { class: String::from("Ljava/lang/String;"), fields: f });
        self.objects.len() as u32
    }

    fn instance_field(&self, handle: u32, fidx: u32) -> R<u32> {
        if handle == 0 {
            return Ok(0);
        }
        let o = self.objects.get(handle as usize - 1).ok_or(InterpErr::BadObject)?;
        Ok(o.fields.get(&fidx).copied().unwrap_or(0))
    }

    fn set_instance_field(&mut self, handle: u32, fidx: u32, v: u32) -> R<()> {
        if handle == 0 {
            return Ok(());
        }
        let o = self.objects.get_mut(handle as usize - 1).ok_or(InterpErr::BadObject)?;
        o.fields.insert(fidx, v);
        Ok(())
    }

    /// 调用轨迹（调试）
    pub fn trace_log(&self) -> Vec<String> {
        self.frames
            .iter()
            .map(|f| {
                let m = self.dex.method(f.mid).unwrap();
                m.signature()
            })
            .collect()
    }

    // ---- native 桩（真实实现的最小集） ----

    fn native_call(&mut self, m: &Method, args: &[u32], widths: &[u32]) -> R<u64> {
        let sig = m.signature();
        let arg = |i: usize| -> R<u32> {
            let mut pos = 0usize;
            for (j, w) in widths.iter().enumerate() {
                if j == i {
                    return Ok(*args.get(pos).unwrap_or(&0));
                }
                pos += *w as usize;
            }
            Err(InterpErr::BadInvoke)
        };
        match sig.as_str() {
            "Ljava/lang/Math;->abs(I)I" => Ok((arg(0)? as i32).unsigned_abs() as u64),
            "Ljava/lang/Math;->max(II)I" => Ok(arg(0)?.max(arg(1)?) as u64),
            "Ljava/lang/Math;->min(II)I" => Ok(arg(0)?.min(arg(1)?) as u64),
            _ => Err(InterpErr::NoSuchMethod),
        }
    }
}

// ---- 解码辅助 ----

fn word(d: &[u8], pc: usize) -> R<u16> {
    if pc + 2 > d.len() {
        return Err(InterpErr::BadCode);
    }
    Ok(u16::from_le_bytes([d[pc], d[pc + 1]]))
}

fn i32_at(d: &[u8], pc: usize) -> R<i32> {
    if pc + 4 > d.len() {
        return Err(InterpErr::BadCode);
    }
    Ok(i32::from_le_bytes([d[pc], d[pc + 1], d[pc + 2], d[pc + 3]]))
}

fn u32_at(d: &[u8], pc: usize) -> R<u32> {
    Ok(i32_at(d, pc)? as u32)
}

fn u64_at(d: &[u8], pc: usize) -> R<u64> {
    if pc + 8 > d.len() {
        return Err(InterpErr::BadCode);
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[pc..pc + 8]);
    Ok(u64::from_le_bytes(b))
}

fn param_widths(params: &[String]) -> Vec<u32> {
    params
        .iter()
        .map(|p| if p.starts_with('J') || p.starts_with('D') { 2 } else { 1 })
        .collect()
}

fn binop_int(op: u8, x: i32, y: i32) -> R<i32> {
    Ok(match op {
        0x91 | 0xd0 | 0xd8 | 0xe0 => x.wrapping_add(y),
        0x92 | 0xd1 => x.wrapping_sub(y),
        0x93 | 0xd2 | 0xd9 | 0xe2 => x.wrapping_mul(y),
        0x94 | 0xd3 | 0xda | 0xe3 => if y == 0 { 0 } else { x.wrapping_div(y) },
        0x95 | 0xd4 | 0xdb | 0xe4 => if y == 0 { 0 } else { x.wrapping_rem(y) },
        0x96 | 0xd5 | 0xdd | 0xe5 => x & y,
        0x97 | 0xd6 | 0xde | 0xe6 => x | y,
        0x98 | 0xd7 | 0xdf | 0xe7 => x ^ y,
        0x99 | 0xe8 => x.wrapping_shl(y as u32 & 31),
        0x9a | 0xe9 => x.wrapping_shr(y as u32 & 31),
        0x9b | 0xea => ((x as u32).wrapping_shr(y as u32 & 31)) as i32,
        _ => return Err(InterpErr::UnsupportedOp(op)),
    })
}

fn binop_lit(op: u8, x: i32, lit: i32) -> R<i32> {
    Ok(match op {
        0xd8 | 0xe0 => x.wrapping_add(lit),
        0xd9 | 0xe1 => lit.wrapping_sub(x),
        0xda | 0xe2 => x.wrapping_mul(lit),
        0xdb | 0xe3 => if lit == 0 { 0 } else { x.wrapping_div(lit) },
        0xdc | 0xe4 => if lit == 0 { 0 } else { x.wrapping_rem(lit) },
        0xdd | 0xe5 => x & lit,
        0xde | 0xe6 => x | lit,
        0xdf | 0xe7 => x ^ lit,
        0xe8 => x.wrapping_shl(lit as u32 & 31),
        0xe9 => x.wrapping_shr(lit as u32 & 31),
        0xea => ((x as u32).wrapping_shr(lit as u32 & 31)) as i32,
        _ => return Err(InterpErr::UnsupportedOp(op)),
    })
}

fn binop_long(op: u8, x: i64, y: i64) -> R<i64> {
    Ok(match op {
        0x9c => x.wrapping_add(y),
        0x9d => x.wrapping_sub(y),
        0x9e => x.wrapping_mul(y),
        0x9f => if y == 0 { 0 } else { x.wrapping_div(y) },
        0xa0 => if y == 0 { 0 } else { x.wrapping_rem(y) },
        0xa1 => x & y,
        0xa2 => x | y,
        0xa3 => x ^ y,
        0xa4 => x.wrapping_shl(y as u32 & 63),
        0xa5 => x.wrapping_shr(y as u32 & 63),
        0xa6 => ((x as u64).wrapping_shr(y as u32 & 63)) as i64,
        _ => return Err(InterpErr::UnsupportedOp(op)),
    })
}

fn str_hash(s: &str) -> u32 {
    let mut h: u32 = 0;
    for b in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u32);
    }
    h
}
