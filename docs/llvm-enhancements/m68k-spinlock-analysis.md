# M68k Spinlock Primitives and Atomic Instructions

**Date**: November 2, 2025 22:50 EET
**Status**: ✅ COMPLETE - TAS atomicity confirmed, BSET/BCLR limitations documented
**LLVM Version**: 19.1.5
**Phase**: 2B - Spinlock Primitives (TAS-Based)

## Executive Summary

The M68k architecture provides the **TAS (Test And Set)** instruction as a hardware-atomic primitive for implementing spinlocks. TAS is **fully atomic** via a locked read-modify-write (RMW) bus cycle and is supported across all M68k processors from MC68000 to MC68060.

**CRITICAL FINDING**: BSET/BCLR bit manipulation instructions are **NOT atomic** for memory operands and cannot be used for spinlock implementation without external synchronization.

---

## TAS (Test And Set) Instruction Specification

### Hardware Atomic Primitive

TAS is the original M68k atomic instruction, present since the MC68000 (1979).

**Source**: M68000 Family Programmer's Reference Manual

### Opcode Encoding

```
15  14  13  12  11  10  9   8   7   6   5         0
+---+---+---+---+---+---+---+---+---+---+-----------+
| 0 | 1 | 0 | 0 | 1 | 0 | 1 | 0 | 1 | 1 |    <ea>   |
+---+---+---+---+---+---+---+---+---+---+-----------+
```

- **Base Opcode**: `0x4AC0`
- **Effective Address** (Bits 5-0): Specifies memory location to test-and-set
- **Operation Size**: Byte (`.B`) only
- **Addressing Modes**: Memory alterable modes (e.g., `(An)`, `(d16,An)`, `(xxx).W`)

**Example**:
```assembly
tas.b (a0)        ; Test and set byte at address in A0
```

### Atomicity Guarantee

**YES - TAS is fully atomic on all M68k processors.**

**Quote** (M68000 Family Programmer's Reference Manual):
> "The operation uses a **locked or read-modify-write transfer sequence**. This instruction supports use of a flag or semaphore to coordinate several processors."

**Mechanism**:
1. **Read Phase**: CPU reads byte from memory
2. **Test Phase**: CPU tests bit 7 (MSB) and sets condition codes (Z/N)
3. **Set Phase**: CPU writes back the byte with bit 7 set (`value | 0x80`)
4. **Bus Lock**: The **entire RMW cycle is atomic** - no other bus master can access memory between read and write

### Bus Locking Behavior

**YES - TAS asserts bus lock signals.**

- **MC68000/010**: Uses **AS** (Address Strobe) and **RMC** (Read-Modify-Cycle) pins
- **MC68020/030/040**: Uses **LOCK** signal (or equivalent internal mechanism)
- **MC68060**: Maintains TAS atomicity via bus locking

**Effect**: During TAS execution, other processors/DMA controllers are prevented from accessing the memory location, ensuring indivisible operation.

### Memory vs Register Operands

**Memory Operands**: TAS is **designed for memory** and provides atomicity **only for memory locations**.

**Register Operands**: TAS can technically operate on data registers (`TAS Dn`), but this is **not useful** for multiprocessor synchronization:
- Register-to-register operations are inherently atomic on a single core
- No bus lock is asserted (local register operation)
- Cannot coordinate with other processors/cores

**Recommended Usage**: Always use TAS with memory operands for spinlock primitives.

### CPU Support Matrix

| CPU      | TAS Support | Atomicity | Notes                          |
| :------- | :---------- | :-------- | :----------------------------- |
| MC68000  | ✅ Yes      | ✅ Atomic | Original atomic primitive      |
| MC68010  | ✅ Yes      | ✅ Atomic | Locked RMW cycle               |
| MC68020  | ✅ Yes      | ✅ Atomic | Enhanced bus locking           |
| MC68030  | ✅ Yes      | ✅ Atomic | Cacheable, locked RMW          |
| MC68040  | ✅ Yes      | ✅ Atomic | Data cache coherency           |
| MC68060  | ✅ Yes      | ✅ Atomic | TAS retained (unlike CAS)      |

**Note**: Unlike CAS/CAS2, TAS was **NOT removed** from MC68060. It remains a fundamental primitive across the entire M68k family.

---

## BSET/BCLR/BCHG/BTST Bit Instructions

### Instruction Overview

The M68k provides four bit manipulation instructions:
- **BSET**: Set bit to 1
- **BCLR**: Clear bit to 0
- **BCHG**: Toggle (change) bit
- **BTST**: Test bit (no modification)

**Implementation**: These instructions are defined in `M68kInstrBits.td`:
```tablegen
defm BCHG : MxBIT<"bchg", 0b101, 0b001>;
defm BCLR : MxBIT<"bclr", 0b110, 0b010>;
defm BSET : MxBIT<"bset", 0b111, 0b011>;
defm BTST : MxBIT<"btst", 0b100, 0b000>;
```

### CRITICAL: BSET/BCLR Are NOT Atomic

**For Memory Operands**: BSET/BCLR perform a **standard read-modify-write sequence WITHOUT bus locking**.

**Race Condition Example**:
```
Scenario: Two CPUs execute BSET and BCLR on the same byte

Timeline:
1. CPU A: Read byte from memory (value = 0b00000001)
2. CPU B: Read byte from memory (value = 0b00000001)  ← Overlapping read!
3. CPU A: Execute BSET #1, calculate new value (0b00000011)
4. CPU B: Execute BCLR #0, calculate new value (0b00000000)
5. CPU A: Write 0b00000011 to memory
6. CPU B: Write 0b00000000 to memory                   ← Lost update!

Result: CPU A's BSET is lost. Expected: 0b00000010, Actual: 0b00000000
```

**Root Cause**: No bus lock is asserted during BSET/BCLR, allowing other bus masters to interleave their own memory accesses.

### When BSET/BCLR Are Safe

**Register Operands**: BSET/BCLR on data registers (`BSET #n, Dn`) are safe for single-threaded use, but do not provide multiprocessor synchronization.

**Protected Context**: BSET/BCLR can be used within a critical section protected by a TAS-based spinlock:
```assembly
spinlock_acquire:
    tas.b   (lock_byte)
    bne.s   spinlock_acquire    ; Retry if already locked

    ; Critical section - now safe to use BSET/BCLR
    bset    #3, (flags_byte)    ; Safe: protected by spinlock

spinlock_release:
    clr.b   (lock_byte)         ; Release lock
```

**Hardware Atomic `BSET`**: **NO M68k CPU provides a hardware-atomic BSET instruction.** For atomic bit modification, use TAS or CAS-based approaches.

---

## Performance Characteristics

### TAS Instruction Timing

**Source**: M68k Family User's Manuals

| Processor | TAS.B (An) | TAS.B (xxx).W | TAS.B (d16,An,Xi) | Notes                 |
| :-------- | :--------- | :------------ | :---------------- | :-------------------- |
| MC68000   | 14 (1/1)   | 18 (1/1)      | 20 (1/1)          | RMW cycle is slow     |
| MC68020   | 9 (1/1)    | 10 (1/1)      | 12 (1/1)          | Cacheable             |
| MC68030   | 7 (1/1)    | 8 (1/1)       | 10 (1/1)          | Cache hit timing      |
| MC68040   | 5 (1/1)    | 6 (1/1)       | 7 (1/1)           | Data cache hit timing |

**(r/w)** notation: (read cycles / write cycles)

**Comparison to CAS**:
- **TAS.B**: 5-14 cycles (byte operation)
- **CAS.W**: 7-16 cycles (word operation)
- **Conclusion**: TAS is slightly faster than CAS for simple test-and-set operations

### Bus Lock Overhead

**Contention Impact**: In a highly contended multiprocessor system, TAS performance is dominated by **bus lock acquisition time**, not the instruction's internal cycle count.

**Lock Duration**: The bus lock is held for the duration of the RMW cycle:
- **MC68000**: ~14 bus cycles (longest)
- **MC68040**: ~5 bus cycles (shortest)

**System-Wide Impact**: During TAS execution, other processors **cannot access the locked memory location**, potentially stalling other cores waiting for bus grant.

---

## Rust Implementation Guidance

### Option A: Inline Assembly Spinlock

**Recommended for NeXTRust Project**: Rust inline assembly provides direct TAS access.

```rust
use core::arch::asm;

/// Atomic test-and-set primitive using M68k TAS instruction
/// CRITICAL: TAS sets CCR flags, use SMI to capture state
#[inline(always)]
pub unsafe fn spin_try_lock(lock: *mut u8) -> bool {
    let mut was_set: u8;
    asm!(
        "tas.b ({addr})",     // atomically set bit7
        "smi {out}",          // out = 0xFF if bit7 was 1 before TAS, else 0x00
        addr = in(reg) lock,
        out = lateout(reg_data) was_set,
        options(nostack)
    );
    was_set == 0  // success if bit7 was previously 0
}

/// Spinlock acquire using TAS
#[inline]
pub fn spin_lock(lock: &mut u8) {
    while !unsafe { spin_try_lock(lock as *mut u8) } {
        core::hint::spin_loop();
    }
}

/// Spinlock release (simple clear)
#[inline]
pub fn spin_unlock(lock: &mut u8) {
    // clear bit7; use a normal store
    unsafe { core::ptr::write_volatile(lock, 0); }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
```

**Usage Example**:
```rust
static mut SPINLOCK: u8 = 0;

fn protected_operation() {
    unsafe {
        spin_lock(&mut SPINLOCK);

        // Critical section
        // ...

        spin_unlock(&mut SPINLOCK);
    }
}
```

### CRITICAL: TAS ≠ Atomic Swap

**IMPORTANT**: TAS and atomic swap (`atomicrmw xchg`) are **fundamentally different operations** and must NOT be conflated.

**TAS Operation**: Test-and-Set **bit 7 only**
- Reads byte from memory
- Sets condition codes (N/Z) based on original value
- Forces bit 7 to `1`, **leaves bits 6-0 untouched**
- Writes modified byte back atomically
- **Does NOT return the original value in a register**

**Atomic Swap Operation**: Replace **entire byte**
- Reads byte from memory
- Writes new **arbitrary value**
- Returns original value atomically

**These are NOT equivalent.** Lowering `atomicrmw xchg` to TAS would corrupt data.

**Example Failure**:
```rust
// Programmer expects to swap entire byte value
let old = atomic_u8.swap(5);  // Expects memory = 5

// If incorrectly lowered to TAS:
// - Memory becomes 0x80 (128), NOT 5
// - Value 5 is completely ignored
// - This is bit manipulation, not value replacement
// - Result: DATA CORRUPTION
```

**Correct Implementation**:
- **TAS**: Use ONLY for byte-flag spinlocks that explicitly use bit 7 as a lock flag
- **Atomic Swap**: Use CAS-loop (020/030/040) or libcall (000/010/060), NEVER TAS

**Why the Confusion?**: The name "Test-And-Set" might suggest it's equivalent to a swap operation, but it's actually "Test-And-Set-**Bit-7**". The operation was designed specifically for simple binary semaphores where bit 7 serves as the lock flag.

### Option B: Rust Atomic Wrapper (Future Work)

**Goal**: Provide `std::sync::atomic::AtomicBool`-compatible interface using TAS

**Challenges**:
1. Rust's atomic primitives expect compare-exchange semantics (CAS-like)
2. TAS is test-and-set only (not compare-exchange)
3. Requires custom atomic lowering in LLVM or Rust's libcore

**Future Path**:
- Modify M68kISelLowering.cpp to detect `atomic_swap` on `i8` types
- Lower to TAS instruction when:
  - Operation is atomic swap on byte
  - Memory ordering is compatible (Acquire/Release/SeqCst)
- Fall back to CAS-based expansion otherwise

**Estimated Effort**: 4-6 hours for LLVM lowering + testing

---

## LLVM Backend Status

### TAS Instruction Implementation

**Current Status**: TAS is **NOT implemented** in the M68k LLVM backend.

**Verification**:
```bash
grep -R "\bTAS\b" src/llvm-project/llvm/lib/Target/M68k/
```
**Result**: No TAS instruction definitions found.

**Implications**:
- Rust's `core::sync::atomic` operations will use CAS-based expansion (for MC68020/030/040) or libcalls (for MC68000/010/060)
- TAS can only be accessed via inline assembly (as shown above)

### BSET/BCLR Implementation

**Current Status**: BSET/BCLR are **fully implemented** in `M68kInstrBits.td`

**Usage**: Available via inline assembly:
```rust
unsafe {
    asm!("bset #3, ({0})", in(reg) ptr, options(nostack));
}
```

**Reminder**: Use only within TAS-protected critical sections.

---

## Future Work

### Phase 3: TAS Instruction Implementation

If TAS lowering is desired:

1. **Add TAS Instruction** (`M68kInstrInfo.td`):
   ```tablegen
   def TAS8 : MxInst<(outs MxType8d.ROp:$dst),
                     (ins MxARI8:$mem),
                     "tas.b $mem">;
   ```

2. **Pattern Matching** (`M68kISelLowering.cpp`):
   ```cpp
   setOperationAction(ISD::ATOMIC_SWAP, MVT::i8, Legal);
   ```

3. **Test Suite**: Create `tas-spinlock.ll` covering all CPU variants

4. **Documentation**: Update atomic support tracker with TAS availability

### Phase 3: Spinlock Library Crate

**Goal**: Provide `nextstep-spinlock` crate with:
- TAS-based `Spinlock<T>` type
- Compatible with `std::sync::Mutex` API
- Optimized for NeXTSTEP on M68k

---

## Summary

| Aspect              | TAS          | BSET/BCLR   | Notes                                  |
| :------------------ | :----------- | :---------- | :------------------------------------- |
| Atomic for Memory   | ✅ Yes       | ❌ No       | TAS uses locked RMW, BSET/BCLR do not  |
| CPU Support         | All (000-060)| All (000-060)| TAS retained even on MC68060           |
| LLVM Implementation | ❌ Absent    | ✅ Present  | TAS requires inline asm currently      |
| Spinlock Use        | ✅ Ideal     | ⚠️ Protected| Use TAS directly, BSET inside critical section |
| Performance (040)   | 5 cycles     | ~3 cycles   | TAS has bus lock overhead              |

---

## References

- **M68000 Family Programmer's Reference Manual**: TAS instruction specification and atomicity guarantee
- **MC68000 User's Manual**: TAS timing characteristics
- **LLVM M68kInstrBits.td**: BSET/BCLR/BCHG/BTST implementation (lines 127-129)
- **Rust Inline Assembly**: [The Rust Reference - Inline Assembly](https://doc.rust-lang.org/reference/inline-assembly.html)

---

## Related Documentation

- **Phase 1 Summary**: `m68k-cas-implementation-summary.md` (CAS hardware gating)
- **Phase 2A CAS2**: `m68k-cas2-analysis.md` (64-bit atomics, CAS2 absence)
- **Phase 2C Performance**: `m68k-cas-performance-analysis.md` (CAS timing comparison)

---

**Implementation**: Claude Code (Sonnet 4.5) working with NeXTRust project
**Reference Material**: M68000 Family Programmer's Reference Manual (TAS specification)
**Validated On**: November 2025 Toolchain (LLVM 19.1.5)
