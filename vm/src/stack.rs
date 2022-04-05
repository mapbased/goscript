use super::gc::GcoVec;
use super::instruction::{Instruction, OpIndex, Opcode, ValueType};
use super::metadata::Meta;
use super::value::*;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt::{self, Display};
use std::ptr;
use std::rc::Rc;
use std::{mem, panic};

const DEFAULT_CAPACITY: usize = 1024;

macro_rules! stack_binary_op {
    ($stack:ident, $op:tt, $t:ident) => {{
        let len = $stack.len();
        let a = $stack.get_c(len - 2);
        let b = $stack.get_c(len - 1);
        *$stack.get_c_mut(len - 2) = a.$op(b, $t);
        $stack.pop_discard();
    }};
}

macro_rules! stack_binary_op_shift {
    ($stack:ident, $op:tt, $t0:ident, $t1:ident) => {{
        let mut right = $stack.pop_c();
        right.to_uint32($t1);
        $stack
            .get_c_mut($stack.len() - 1)
            .$op(right.as_uint32(), $t0);
    }};
}

macro_rules! stack_cmp_op {
    ($stack:ident, $op:tt, $t:ident) => {{
        let len = $stack.len();
        let a = $stack.get_c(len - 2);
        let b = $stack.get_c(len - 1);
        *$stack.get_c_mut(len - 2) = Value64::from_bool(Value64::$op(a, b, $t));
        $stack.pop_discard();
    }};
}

macro_rules! read_with_ops {
    ($op:expr, $lhs:expr, $rhs:expr, $shift_rhs:expr, $t:expr) => {{
        match $op {
            Opcode::UNARY_ADD => {
                let mut v = $lhs.clone();
                v.inc($t);
                v
            } // INC
            Opcode::UNARY_SUB => {
                let mut v = $lhs.clone();
                v.dec($t);
                v
            } // DEC
            Opcode::ADD => $lhs.binary_op_add($rhs, $t),
            Opcode::SUB => $lhs.binary_op_sub($rhs, $t),
            Opcode::MUL => $lhs.binary_op_mul($rhs, $t),
            Opcode::QUO => $lhs.binary_op_quo($rhs, $t),
            Opcode::REM => $lhs.binary_op_rem($rhs, $t),
            Opcode::AND => $lhs.binary_op_and($rhs, $t),
            Opcode::OR => $lhs.binary_op_or($rhs, $t),
            Opcode::XOR => $lhs.binary_op_xor($rhs, $t),
            Opcode::AND_NOT => $lhs.binary_op_and_not($rhs, $t),
            Opcode::SHL => {
                let mut v = $lhs.clone();
                v.binary_op_shl($shift_rhs.as_uint32(), $t);
                v
            }
            Opcode::SHR => {
                let mut v = $lhs.clone();
                v.binary_op_shr($shift_rhs.as_uint32(), $t);
                v
            }
            _ => unreachable!(),
        }
    }};
}

macro_rules! store_local_val {
    ($stack:ident, $to:expr, $s_index:ident, $r_index:ident, $t:ident, $gcos:ident) => {{
        if $r_index < 0 {
            if $t.copyable() {
                *$to.get_c_mut($s_index) = $stack.read_copyable($r_index);
            } else {
                *$to.get_rc_mut($s_index) = $stack.read_non_copyable($r_index, $gcos);
            }
        } else {
            if $t.copyable() {
                *$to.get_c_mut($s_index) =
                    $stack.read_copyable_ops($to.get_c($s_index), $r_index, $t);
            } else {
                *$to.get_rc_mut($s_index) =
                    $stack.read_non_copyable_ops($to.get_rc($s_index), $r_index, $t);
            }
        }
    }};
}

// NOTE! the type-tag of copyable values are not properly maintained
// Make sure non-copyable data are properly released
pub struct Stack {
    v: Vec<GosValue>,
}

impl Display for Stack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl fmt::Debug for Stack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("=====rc  top=======\n")?;
        for i in 0..(self.v.len()) {
            write!(f, "{}\n", &self.v[i])?;
        }
        f.write_str("=====rc  botton====\n")
    }
}

impl Stack {
    #[inline]
    pub fn new() -> Stack {
        Stack {
            v: Vec::with_capacity(DEFAULT_CAPACITY),
        }
    }

    #[inline]
    pub fn with_data(mut vec: Vec<GosValue>) -> Stack {
        if vec.len() < DEFAULT_CAPACITY {
            vec.reserve(DEFAULT_CAPACITY - vec.len());
        }
        Stack { v: vec }
    }

    #[inline]
    pub fn move_from(other: &mut Stack, count: usize) -> Stack {
        let v = other.pop_n(count);
        Stack::with_data(v)
    }

    pub fn pop_n(&mut self, n: usize) -> Vec<GosValue> {
        let mid = self.len() - n;
        self.v.split_off(mid)
    }

    pub fn push_n(&mut self, mut vec: Vec<GosValue>) {
        self.v.append(&mut vec)
    }

    #[inline]
    pub fn push(&mut self, val: GosValue) {
        self.v.push(val);
    }

    #[inline]
    pub fn push_from_index(&mut self, index: usize, t: ValueType) {
        self.push(self.get_rc(index).clone());
    }

    #[inline]
    pub fn push_nil(&mut self) {
        self.push(GosValue::new_nil());
    }

    #[inline]
    pub fn push_bool(&mut self, b: bool) {
        self.push(GosValue::new_bool(b));
    }

    #[inline]
    pub fn push_int(&mut self, i: isize) {
        self.push(GosValue::new_int(i));
    }

    #[inline]
    pub fn push_int32_as(&mut self, i: i32, t: ValueType) {
        self.push(GosValue::int32_as(i, t))
    }

    #[inline]
    pub fn pop_discard(&mut self) {
        self.pop_discard_n(1);
    }

    #[inline]
    pub fn pop_discard_n(&mut self, n: usize) {
        self.v.truncate(self.len() - n);
    }

    #[inline]
    pub fn pop_c(&mut self) -> Value64 {
        let v = self.get_c(self.len() - 1).clone();
        self.pop_discard();
        v
    }

    #[inline]
    pub fn pop_rc(&mut self) -> GosValue {
        self.v.pop().unwrap()
    }

    #[inline]
    pub fn pop_with_type(&mut self, t: ValueType) -> GosValue {
        self.pop_rc()
    }

    #[inline]
    pub fn pop_with_type_n(&mut self, types: &[ValueType]) -> Vec<GosValue> {
        self.pop_n(types.len())
    }

    #[inline]
    pub fn pop_interface(&mut self) -> Rc<RefCell<InterfaceObj>> {
        let ret = self.pop_rc();
        match ret {
            GosValue::Interface(i) => i,
            GosValue::Named(n) => match &n.0 {
                GosValue::Interface(i) => i.clone(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }

    #[inline]
    pub fn pop_bool(&mut self) -> bool {
        let v = *self.get_c(self.len() - 1).as_bool();
        self.pop_discard();
        v
    }

    #[inline]
    pub fn pop_int(&mut self) -> isize {
        let v = *self.get_c(self.len() - 1).as_int();
        self.pop_discard();
        v
    }

    #[inline]
    pub fn pop_int32(&mut self) -> i32 {
        let v = *self.get_c(self.len() - 1).as_int32();
        self.pop_discard();
        v
    }

    #[inline]
    pub fn pop_uint(&mut self) -> usize {
        let v = *self.get_c(self.len() - 1).as_uint();
        self.pop_discard();
        v
    }

    #[inline]
    pub fn pop_uint32(&mut self) -> u32 {
        let v = *self.get_c(self.len() - 1).as_uint32();
        self.pop_discard();
        v
    }

    #[inline]
    pub fn get_with_type(&self, index: usize, t: ValueType) -> GosValue {
        if t.copyable() {
            self.get_c(index).v128(t)
        } else {
            self.get_rc(index).clone()
        }
    }

    #[inline]
    pub fn copy_semantic_with_type(&self, index: usize, t: ValueType, gcv: &GcoVec) -> GosValue {
        if t.copyable() {
            self.get_c(index).v128(t)
        } else {
            self.get_rc(index).copy_semantic(gcv)
        }
    }

    #[inline]
    pub fn set(&mut self, index: usize, val: GosValue) -> ValueType {
        let t = val.typ();
        *self.get_rc_mut(index) = val;
        t
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.v.len()
    }

    #[inline]
    pub fn truncate(&mut self, len: usize) {
        self.v.truncate(len);
    }

    #[inline]
    pub fn append(&mut self, mut vec: Vec<GosValue>) {
        for v in vec.drain(..) {
            self.push(v);
        }
    }

    #[inline]
    pub fn split_off_with_type(&mut self, index: usize, t: ValueType) -> Vec<GosValue> {
        self.v.split_off(index)
    }

    #[inline]
    fn read_copyable(&self, r_index: OpIndex) -> Value64 {
        let rhs_s_index = Stack::offset(self.len(), r_index);
        *self.get_c(rhs_s_index)
    }

    #[inline]
    fn read_non_copyable(&self, r_index: OpIndex, gcos: &GcoVec) -> GosValue {
        let rhs_s_index = Stack::offset(self.len(), r_index);
        self.get_rc(rhs_s_index).copy_semantic(gcos)
    }

    #[inline]
    fn read_non_copyable_ops(&self, lhs: &GosValue, r_index: OpIndex, t: ValueType) -> GosValue {
        let ri = Stack::offset(self.len(), -1);
        if t == ValueType::Named {
            let op = Instruction::index2code(r_index);
            let l = lhs.as_named();
            let (a, t) = Value64::from_v128(&l.0).unwrap();
            let v = read_with_ops!(
                op,
                &a,
                &Value64::from_v128(&self.get_rc(ri).as_named().0).unwrap().0,
                self.get_c(ri),
                t
            );
            GosValue::Named(Box::new((v.v128(t), l.1)))
        } else {
            GosValue::add_str(lhs, self.get_rc(ri))
        }
    }

    #[inline]
    fn read_copyable_ops(&self, lhs: &Value64, r_index: OpIndex, t: ValueType) -> Value64 {
        let ri = Stack::offset(self.len(), -1);
        let op = Instruction::index2code(r_index);
        read_with_ops!(op, lhs, self.get_c(ri), self.get_c(ri), t)
    }

    #[inline]
    pub fn store_val(&self, target: &mut GosValue, r_index: OpIndex, t: ValueType, gcos: &GcoVec) {
        *target = if r_index < 0 {
            if t.copyable() {
                self.read_copyable(r_index).v128(t)
            } else {
                self.read_non_copyable(r_index, gcos)
            }
        } else {
            if t.copyable() {
                self.read_copyable_ops(&Value64::from_v128(target).unwrap().0, r_index, t)
                    .v128(t)
            } else {
                self.read_non_copyable_ops(target, r_index, t)
            }
        };
    }

    #[inline]
    pub fn store_local(&mut self, s_index: usize, r_index: OpIndex, t: ValueType, gcos: &GcoVec) {
        store_local_val!(self, self, s_index, r_index, t, gcos);
    }

    #[inline]
    pub fn store_up_value(
        &mut self,
        upvalue: &UpValue,
        rhs_index: OpIndex,
        typ: ValueType,
        gcos: &GcoVec,
    ) {
        match &mut upvalue.inner.borrow_mut() as &mut UpValueState {
            UpValueState::Open(desc) => {
                let index = (desc.stack_base + desc.index) as usize;
                let uv_stack = desc.stack.upgrade().unwrap();
                match ptr::eq(uv_stack.as_ptr(), self) {
                    true => store_local_val!(self, self, index, rhs_index, typ, gcos),
                    false => store_local_val!(
                        self,
                        &mut uv_stack.borrow_mut(),
                        index,
                        rhs_index,
                        typ,
                        gcos
                    ),
                };
            }
            UpValueState::Closed(v) => {
                self.store_val(v, rhs_index, typ, gcos);
            }
        }
    }

    #[inline]
    pub fn store_index(
        &self,
        target: &GosValue,
        key: &GosValue,
        r_index: OpIndex,
        t: ValueType,
        gcos: &GcoVec,
    ) -> RuntimeResult<()> {
        let err = Err("assignment to entry in nil map or slice".to_owned());
        match target {
            GosValue::Array(arr) => {
                let target_cell = &arr.0.borrow_data()[*key.as_int() as usize];
                self.store_val(&mut target_cell.borrow_mut(), r_index, t, gcos);
                Ok(())
            }
            GosValue::Slice(s) => {
                let target_cell = &s.0.borrow()[*key.as_int() as usize];
                self.store_val(&mut target_cell.borrow_mut(), r_index, t, gcos);
                Ok(())
            }
            GosValue::Map(map) => {
                map.0.touch_key(&key);
                let borrowed = map.0.borrow_data();
                let target_cell = borrowed.get(&key).unwrap();
                self.store_val(&mut target_cell.borrow_mut(), r_index, t, gcos);
                Ok(())
            }
            GosValue::Nil(_) => err,
            _ => {
                dbg!(target);
                unreachable!()
            }
        }
    }

    #[inline]
    pub fn store_index_int(
        &self,
        target: &GosValue,
        i: OpIndex,
        r_index: OpIndex,
        t: ValueType,
        gcos: &GcoVec,
    ) -> RuntimeResult<()> {
        let index = GosValue::new_int(i as isize);
        self.store_index(target, &index, r_index, t, gcos)
    }

    #[inline]
    pub fn store_field(
        &self,
        target: &GosValue,
        key: &GosValue,
        r_index: OpIndex,
        t: ValueType,
        metas: &MetadataObjs,
        gcos: &GcoVec,
    ) {
        match target {
            GosValue::Struct(s) => {
                match key {
                    GosValue::Int(i) => {
                        let target = &mut s.0.borrow_mut().fields[*i.as_int() as usize];
                        self.store_val(target, r_index, t, gcos);
                    }
                    GosValue::Str(sval) => {
                        let i = s.0.borrow().meta.field_index(sval.as_str(), metas);
                        let target = &mut s.0.borrow_mut().fields[i as usize];
                        self.store_val(target, r_index, t, gcos);
                    }
                    _ => unreachable!(),
                };
            }
            _ => unreachable!(),
        }
    }

    pub fn store_to_pointer(
        &mut self,
        p: &PointerObj,
        rhs_index: OpIndex,
        typ: ValueType,
        packages: &PackageObjs,
        gcv: &GcoVec,
    ) {
        match p {
            PointerObj::UpVal(uv) => {
                self.store_up_value(uv, rhs_index, typ, gcv);
            }
            PointerObj::Struct(r, _) => {
                let rhs_s_index = Stack::offset(self.len(), rhs_index);
                let val = self
                    .get_with_type(rhs_s_index, typ)
                    .unwrap_named()
                    .copy_semantic(gcv);
                let mref: &mut StructObj = &mut r.0.borrow_mut();
                *mref = val.as_struct().0.borrow().clone();
            }
            PointerObj::Array(a, _) => {
                let rhs_s_index = Stack::offset(self.len(), rhs_index);
                let val = self.get_with_type(rhs_s_index, typ);
                a.0.set_from(&val.as_array().0);
            }
            PointerObj::Slice(r, _) => {
                let rhs_s_index = Stack::offset(self.len(), rhs_index);
                let val = self.get_with_type(rhs_s_index, typ);
                r.0.set_from(&val.as_slice().0);
            }
            PointerObj::Map(m, _) => {
                let rhs_s_index = Stack::offset(self.len(), rhs_index);
                let val = self.get_with_type(rhs_s_index, typ);
                let mref: &mut GosHashMap = &mut m.0.borrow_data_mut();
                *mref = val.as_map().0.borrow_data().clone();
            }
            PointerObj::SliceMember(s, index) => {
                let vborrow = s.0.borrow();
                let target: &mut GosValue =
                    &mut vborrow[s.0.begin() + *index as usize].borrow_mut();
                self.store_val(target, rhs_index, typ, gcv);
            }
            PointerObj::StructField(s, index) => {
                let target: &mut GosValue = &mut s.0.borrow_mut().fields[*index as usize];
                self.store_val(target, rhs_index, typ, gcv);
            }
            PointerObj::PkgMember(p, index) => {
                let target: &mut GosValue = &mut packages[*p].member_mut(*index);
                self.store_val(target, rhs_index, typ, gcv);
            }
        };
    }

    #[inline]
    pub fn push_index_comma_ok(&mut self, map: &GosValue, index: &GosValue) {
        let (v, b) = match map.as_map().0.try_get(index) {
            Some(v) => (v, true),
            None => (GosValue::new_nil(), false),
        };
        self.push(v);
        self.push_bool(b);
    }

    #[inline]
    pub fn pack_variadic(&mut self, index: usize, meta: Meta, t: ValueType, gcos: &GcoVec) {
        if index <= self.len() {
            let mut v = Vec::new();
            v.append(&mut self.split_off_with_type(index, t));
            self.push(GosValue::slice_with_val(v, meta, gcos))
        }
    }

    #[inline]
    pub fn init_pkg_vars(&mut self, pkg: &PackageVal, count: usize) {
        for i in 0..count {
            let var_index = (count - 1 - i) as OpIndex;
            let var: &mut GosValue = &mut pkg.var_mut(var_index);
            let t = var.typ();
            *var = self.pop_with_type(t);
        }
    }

    #[inline]
    pub fn unwrap_named(&mut self, i: usize) -> ValueType {
        self.set(i, self.get_rc(i).as_named().0.clone())
    }

    #[inline]
    pub fn wrap_restore_named(&mut self, i: usize, typ: ValueType) {
        let val = self.get_with_type(i, typ);
        match self.get_rc_mut(i) {
            GosValue::Named(n) => {
                n.as_mut().0 = val;
            }
            _ => {
                dbg!(self.get_rc_mut(i));
                unreachable!();
            }
        }
    }

    #[inline]
    pub fn add(&mut self, t: ValueType) {
        if t.copyable() {
            stack_binary_op!(self, binary_op_add, t)
        } else {
            match t {
                ValueType::Str => {
                    let a = self.get_rc(self.len() - 2);
                    let b = self.get_rc(self.len() - 1);
                    *self.get_rc_mut(self.len() - 2) = GosValue::add_str(a, b);
                    self.pop_discard();
                }
                ValueType::Named => {
                    let t = self.unwrap_named(self.len() - 2);
                    self.unwrap_named(self.len() - 1);
                    stack_binary_op!(self, binary_op_add, t);
                    self.wrap_restore_named(self.len() - 1, t)
                }
                _ => unreachable!(),
            }
        }
    }

    #[inline]
    pub fn switch_cmp(&mut self, t: ValueType, objs: &VMObjects) -> bool {
        let b = if t.copyable() {
            let len = self.len();
            let a = self.get_c(len - 2);
            let b = self.get_c(len - 1);
            Value64::compare_eql(a, b, t)
        } else {
            let a = self.get_rc(self.len() - 2);
            let b = self.get_rc(self.len() - 1);
            if t != ValueType::Metadata {
                a.eq(&b)
            } else {
                a.as_meta().identical(b.as_meta(), &objs.metas)
            }
        };
        self.pop_discard();
        b
    }

    #[inline]
    pub fn sub(&mut self, t: ValueType) {
        stack_binary_op!(self, binary_op_sub, t)
    }

    #[inline]
    pub fn mul(&mut self, t: ValueType) {
        stack_binary_op!(self, binary_op_mul, t)
    }

    #[inline]
    pub fn quo(&mut self, t: ValueType) {
        stack_binary_op!(self, binary_op_quo, t)
    }

    #[inline]
    pub fn rem(&mut self, t: ValueType) {
        stack_binary_op!(self, binary_op_rem, t)
    }

    #[inline]
    pub fn and(&mut self, t: ValueType) {
        stack_binary_op!(self, binary_op_and, t)
    }

    #[inline]
    pub fn or(&mut self, t: ValueType) {
        stack_binary_op!(self, binary_op_or, t)
    }

    #[inline]
    pub fn xor(&mut self, t: ValueType) {
        stack_binary_op!(self, binary_op_xor, t)
    }

    #[inline]
    pub fn shl(&mut self, t0: ValueType, t1: ValueType) {
        stack_binary_op_shift!(self, binary_op_shl, t0, t1)
    }

    #[inline]
    pub fn shr(&mut self, t0: ValueType, t1: ValueType) {
        stack_binary_op_shift!(self, binary_op_shr, t0, t1)
    }

    #[inline]
    pub fn and_not(&mut self, t: ValueType) {
        stack_binary_op!(self, binary_op_and_not, t)
    }

    #[inline]
    pub fn unary_negate(&mut self, t: ValueType) {
        self.get_c_mut(self.len() - 1).unary_negate(t);
    }

    #[inline]
    pub fn unary_xor(&mut self, t: ValueType) {
        self.get_c_mut(self.len() - 1).unary_xor(t);
    }

    #[inline]
    pub fn logical_not(&mut self, t: ValueType) {
        self.get_c_mut(self.len() - 1).unary_not(t);
    }

    #[inline]
    pub fn compare_eql(&mut self, t: ValueType) {
        if t.copyable() {
            stack_cmp_op!(self, compare_eql, t);
        } else {
            let (b, a) = (self.pop_with_type(t), self.pop_with_type(t));
            self.push_bool(a.eq(&b));
        }
    }

    #[inline]
    pub fn compare_neq(&mut self, t: ValueType) {
        if t.copyable() {
            stack_cmp_op!(self, compare_neq, t);
        } else {
            let (b, a) = (self.pop_with_type(t), self.pop_with_type(t));
            self.push_bool(!a.eq(&b));
        }
    }

    #[inline]
    pub fn compare_lss(&mut self, t: ValueType) {
        if t.copyable() {
            stack_cmp_op!(self, compare_lss, t);
        } else {
            let (b, a) = (self.pop_with_type(t), self.pop_with_type(t));
            self.push_bool(a.cmp(&b) == Ordering::Less);
        }
    }

    #[inline]
    pub fn compare_gtr(&mut self, t: ValueType) {
        if t.copyable() {
            stack_cmp_op!(self, compare_gtr, t);
        } else {
            let (b, a) = (self.pop_with_type(t), self.pop_with_type(t));
            self.push_bool(a.cmp(&b) == Ordering::Greater);
        }
    }

    #[inline]
    pub fn compare_leq(&mut self, t: ValueType) {
        if t.copyable() {
            stack_cmp_op!(self, compare_leq, t);
        } else {
            let (b, a) = (self.pop_with_type(t), self.pop_with_type(t));
            self.push_bool(a.cmp(&b) != Ordering::Greater);
        }
    }

    #[inline]
    pub fn compare_geq(&mut self, t: ValueType) {
        if t.copyable() {
            stack_cmp_op!(self, compare_geq, t);
        } else {
            let (b, a) = (self.pop_with_type(t), self.pop_with_type(t));
            self.push_bool(a.cmp(&b) != Ordering::Less);
        }
    }

    #[inline]
    pub fn get_c(&self, i: usize) -> &Value64 {
        let t = self.get_rc(i).typ();
        if !t.copyable() && t != ValueType::Nil {
            dbg!(t, self.get_rc(i));
            panic!("11111");
        }
        unsafe {
            // one word offset from beginning
            let p: *const usize = mem::transmute(self.v.get_unchecked(i));
            mem::transmute(p.offset(1))
        }
    }

    #[inline]
    pub fn get_c_mut(&mut self, i: usize) -> &mut Value64 {
        let t = self.get_rc(i).typ();
        if !t.copyable() && t != ValueType::Nil {
            dbg!(t);
            dbg!(self.get_rc(i));
            panic!("22222");
        }
        unsafe {
            // one word offset from beginning
            let p: *mut usize = mem::transmute(self.v.get_unchecked_mut(i));
            mem::transmute(p.offset(1))
        }
    }

    #[inline]
    pub fn get_rc(&self, i: usize) -> &GosValue {
        unsafe { self.v.get_unchecked(i) }
    }

    #[inline]
    pub fn get_rc_mut(&mut self, i: usize) -> &mut GosValue {
        unsafe { self.v.get_unchecked_mut(i) }
    }

    #[inline]
    pub fn offset(base: usize, offset: OpIndex) -> usize {
        (base as isize + offset as isize) as usize
    }
}

/// store iterators for Opcode::RANGE
pub struct RangeStack {
    maps: Vec<GosHashMapIter<'static>>,
    slices: Vec<SliceEnumIter<'static>>,
    strings: Vec<StringEnumIter<'static>>,
}

impl RangeStack {
    pub fn new() -> RangeStack {
        RangeStack {
            maps: vec![],
            slices: vec![],
            strings: vec![],
        }
    }

    pub fn range_init(&mut self, target: &GosValue) {
        match target {
            GosValue::Map(m) => {
                let map = m.0.borrow_data();
                let iter = unsafe { mem::transmute(map.iter()) };
                self.maps.push(iter);
            }
            GosValue::Slice(sl) => {
                let slice = sl.0.borrow();
                let iter = unsafe { mem::transmute(slice.iter().enumerate()) };
                self.slices.push(iter);
            }
            GosValue::Str(s) => {
                let iter = unsafe { mem::transmute(s.iter().enumerate()) };
                self.strings.push(iter);
            }
            _ => unreachable!(),
        }
    }

    pub fn range_body(&mut self, typ: ValueType, stack: &mut Stack) -> bool {
        match typ {
            ValueType::Map => match self.maps.last_mut().unwrap().next() {
                Some((k, v)) => {
                    stack.push(k.clone());
                    stack.push(v.clone().into_inner());
                    false
                }
                None => {
                    self.maps.pop();
                    true
                }
            },
            ValueType::Slice => match self.slices.last_mut().unwrap().next() {
                Some((k, v)) => {
                    stack.push_int(k as isize);
                    stack.push(v.clone().into_inner());
                    false
                }
                None => {
                    self.slices.pop();
                    true
                }
            },
            ValueType::Str => match self.strings.last_mut().unwrap().next() {
                Some((k, v)) => {
                    stack.push_int(k as isize);
                    stack.push_int(v as isize);
                    false
                }
                None => {
                    self.strings.pop();
                    true
                }
            },
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn test_stack() {
        let mut s = Stack::new();
        //s.push(GosValue::Int(1));
        //assert_eq!(s.pop(), GosValue::Int(1));

        s.push(GosValue::new_str("11".to_owned()));
        let v2 = GosValue::new_str("aa".to_owned());
        s.set(0, v2.clone());
        //assert_eq!(s.get(0, ValueType::Str), v2);
    }
}
