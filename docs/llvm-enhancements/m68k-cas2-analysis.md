# M68k CAS2 Instruction Analysis

**Date**: November 2, 2025 22:45 EET
**Status**: ✅ COMPLETE - CAS2 confirmed absent from LLVM M68k backend
**LLVM Version**: 19.1.5
**Phase**: 2A - CAS2 Investigation & 64-bit Atomic Support

## Executive Summary

The CAS2 (Compare-And-Swap Dual Operands) instruction is **NOT implemented** in the LLVM M68k backend. All 64-bit atomic operations lower to libcalls (`__sync_val_compare_and_swap_8` or `__atomic_compare_exchange_8`), regardless of CPU variant.

This document provides the hardware specification for CAS2, explains why it is not implemented, and outlines a potential future implementation path.

---

## Hardware Specification

### CAS2 Instruction Overview

CAS2 is the dual-operand variant of CAS, designed for atomically updating two distinct memory locations in a single indivisible operation. This is critical for operations like doubly-linked list manipulations where both forward and backward pointers must be updated atomically.

**Source**: M68000 Family Programmer's Reference Manual

### Opcode Encoding

CAS2 uses a three-word instruction format:

**First Word (Opcode)**:
```
15  14  13  12  11  10  9   8   7   6   5   4   3   2   1   0
+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
| 0 | 0 | 0 | 0 | 1 | SIZE  | 0 | 1 | 1 | 1 | 1 | 1 | 1 | 0 | 0 |
+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
```
- **Base Opcode**: `0x08FC`
- **Size Field** (Bits 10-9):
  - `10`: Word (`.W`) operation
  - `11`: Longword (`.L`) operation
  - *Note: Byte size is NOT supported for CAS2*

**Second Word (First Operand)**:
```
15    14-12  11-9   8-6   5-3   2-0
+-----+------+-----+-----+-----+-----+
| D/A1|  Rn1   | 000 | Du1 | 000 | Dc1 |
+-----+------+-----+-----+-----+-----+
```

**Third Word (Second Operand)**:
```
15    14-12  11-9   8-6   5-3   2-0
+-----+------+-----+-----+-----+-----+
| D/A2|  Rn2   | 000 | Du2 | 000 | Dc2 |
+-----+------+-----+-----+-----+-----+
```

**Field Definitions**:
- `D/A1`, `D/A2`: Specifies if `Rn` is a Data (0) or Address (1) register
- `Rn1`, `Rn2`: Register numbers for pointers to the two memory locations
- `Du1`, `Du2`: Update data registers (values to write if comparison succeeds)
- `Dc1`, `Dc2`: Compare data registers (expected values)

### Memory Alignment Requirements

**Natural alignment is required** for atomicity guarantees:
- **Word (`.W`)**: Operands must be on even-byte (2-byte) boundaries
- **Longword (`.L`)**: Operands must be on 4-byte boundaries

Misaligned access will either:
1. Trigger an **address error exception** (MC68000/010)
2. Break atomicity by requiring multiple bus cycles (later CPUs)

### CPU Support Matrix

| CPU       | CAS2 Support    | Rationale                                                        |
| :-------- | :-------------- | :--------------------------------------------------------------- |
| MC68000   | ❌ No           | Pre-CAS architecture                                             |
| MC68010   | ❌ No           | Pre-CAS architecture                                             |
| **MC68020**   | **✅ Yes (First)** | Introduced alongside CAS                                         |
| MC68030   | ✅ Yes          | Includes CAS2 from MC68020                                       |
| MC68040   | ✅ Yes          | Includes CAS2 from MC68020                                       |
| **MC68060**   | **❌ No (Removed)** | **CAS2 removed from hardware** (Vector 61 trap, like CAS)        |

**Hardware Behavior**: On MC68060, executing CAS2 triggers Vector 61 (Unimplemented Integer Instruction Exception), requiring software emulation.

---

## LLVM Backend Implementation Status

### Verification Results

**Command Run**:
```bash
grep -R "CAS2" src/llvm-project/llvm/lib/Target/M68k/
```

**Result**:
```
src/llvm-project/llvm/lib/Target/M68k/M68k.td:62:
    "Target has hardware CAS/CAS2 instructions",
```

**Analysis**: CAS2 is mentioned **only in the feature description comment** for `FeatureHardwareCAS`. No actual instruction definitions, patterns, or lowering logic exist.

### 64-bit Atomic Operation Lowering

**Test File**: `llvm/test/CodeGen/M68k/atomic-64bit.ll`

**Code Generation Results**:
```assembly
cas64:                                  ; @cas64
    suba.l  #36, %sp
    move.l  (48,%sp), (28,%sp)
    move.l  (44,%sp), (24,%sp)
    lea     (24,%sp), %a0
    move.l  %a0, (4,%sp)
    move.l  #0, (20,%sp)
    move.l  #0, (16,%sp)
    move.l  (56,%sp), (12,%sp)
    move.l  (52,%sp), (8,%sp)
    move.l  (40,%sp), (%sp)
    jsr     __atomic_compare_exchange_8  ; ← LIBCALL
    move.l  (28,%sp), %d1
    move.l  (24,%sp), %d0
    adda.l  #36, %sp
    rts
```

**Key Observations**:
1. ✅ Generates `jsr __atomic_compare_exchange_8` libcall
2. ✅ **NO** `cas2` instruction emitted
3. ✅ Behavior is **identical across all 6 CPU variants** (000/010/020/030/040/060)

### Files Searched

The following M68k backend files were examined for CAS2 references:
- `M68k.td` - Feature definition only (comment)
- `M68kInstrAtomics.td` - NO CAS2 patterns
- `M68kISelLowering.cpp` - NO CAS2 lowering logic
- `M68kInstrInfo.td` - NO CAS2 instruction definitions

---

## Rationale for Non-Implementation

### 1. Low Rust Usage Priority

Rust's atomic operations typically focus on single-location atomics (`AtomicU64`, `AtomicPtr`). Dual-location atomic updates are rare and usually handled via higher-level synchronization primitives (mutexes, RwLock).

### 2. Complex Register Allocation

CAS2 requires coordinating **six registers simultaneously**:
- Two address registers (Rn1, Rn2) for memory locations
- Two compare registers (Dc1, Dc2) for expected values
- Two update registers (Du1, Du2) for new values

LLVM's register allocator would need special constraints to prevent overlapping allocations.

### 3. MC68060 Hardware Removal

The most performant M68k CPU (MC68060) **removed CAS2 from hardware**. Any code emitting CAS2 would trap to software emulation on 060, negating performance benefits.

### 4. Alternative Solutions

Modern LLVM atomics provide:
- **Libcalls**: `__sync_*` and `__atomic_*` functions handle 64-bit atomics portably
- **Sequencing**: Multiple CAS operations can be combined with proper memory ordering
- **Lock-free alternatives**: Intrusive data structures can avoid dual-location updates

---

## Future Implementation Path

If CAS2 support is desired, the following steps would be required:

### Phase 1: Instruction Definition
1. Add `CAS2W` and `CAS2L` instruction definitions in `M68kInstrAtomics.td`
2. Define encoding using multi-word format (3 words total)
3. Add predicate to enable only for MC68020/030/040

### Phase 2: Register Allocation Constraints
1. Create special register class for CAS2 operations
2. Add constraints ensuring no overlap between:
   - Rn1/Rn2 (address registers)
   - Dc1/Dc2 (compare registers)
   - Du1/Du2 (update registers)

### Phase 3: IR Lowering
1. Detect LLVM IR patterns for dual-location CAS
2. Lower to CAS2 instruction when:
   - Target CPU supports hardware CAS2 (020/030/040)
   - Register allocation constraints can be satisfied
3. Fall back to libcalls otherwise

### Phase 4: Testing
1. Create comprehensive lit tests covering:
   - Word and longword sizes
   - All addressing modes
   - Success and failure paths
2. Verify MC68060 **does NOT** emit CAS2 (trap avoidance)

### Estimated Effort
**Time**: 8-12 hours for full implementation
**Complexity**: High (register allocation, multi-word encoding, testing)
**Priority**: Low (Rust rarely needs dual-location atomics)

---

## Current Status Summary

| Aspect               | Status       | Details                                    |
| :------------------- | :----------- | :----------------------------------------- |
| Hardware Spec        | ✅ Documented | Opcode 0x08FC, 3-word encoding             |
| CPU Support          | ✅ Documented | MC68020/030/040 only, NOT MC68060          |
| LLVM Implementation  | ❌ Absent     | Mentioned in comment only                  |
| 64-bit Atomics       | ✅ Working    | Libcalls used for all CPU variants         |
| Test Coverage        | ✅ Complete   | atomic-64bit.ll verifies libcall behavior  |
| Future Path          | ✅ Defined    | 4-phase implementation plan documented     |

---

## References

- **M68000 Family Programmer's Reference Manual**: CAS2 instruction specification
- **MC68060 User's Manual**: CAS2 removal and Vector 61 trap behavior
- **LLVM TableGen Documentation**: Multi-word instruction encoding
- **Test Case**: `llvm/test/CodeGen/M68k/atomic-64bit.ll`

---

## Related Documentation

- **Phase 1 Summary**: `m68k-cas-implementation-summary.md` (CAS hardware gating)
- **Phase 1 Completion**: `m68k-cas-phase1-completion.md`
- **Phase 2 Spinlocks**: `m68k-spinlock-analysis.md` (TAS-based primitives)
- **Phase 2 Performance**: `m68k-cas-performance-analysis.md`

---

**Implementation**: Claude Code (Sonnet 4.5) working with NeXTRust project
**Verification**: Manual grep verification + 64-bit atomic test validation
**Validated On**: November 2025 Toolchain (LLVM 19.1.5)
