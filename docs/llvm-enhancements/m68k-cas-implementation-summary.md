# M68k CAS Hardware Feature Implementation Summary

**Date**: November 2, 2025  
**Status**: ✅ COMPLETE - All tests passing  
**LLVM Version**: 19.1.5

## Problem Statement

The M68k LLVM backend incorrectly assumed all M68020+ CPUs support hardware CAS (Compare-And-Swap) instructions. However, the MC68060 removed CAS from hardware - executing these instructions on MC68060 triggers a Vector 61 (Unimplemented Integer Instruction Exception).

## Solution Implemented

Introduced a new `FeatureHardwareCAS` SubtargetFeature that precisely controls CAS instruction availability based on actual M68k hardware specifications.

### Hardware Specification Matrix

| CPU      | ISA Level | Hardware CAS | Rationale                          |
|----------|-----------|--------------|-------------------------------------|
| MC68000  | ISA00     | ❌ No        | Pre-CAS architecture                |
| MC68010  | ISA10     | ❌ No        | Pre-CAS architecture                |
| MC68020  | ISA20     | ✅ Yes       | First CPU with CAS instructions    |
| MC68030  | ISA30     | ✅ Yes       | Includes CAS from MC68020          |
| MC68040  | ISA40     | ✅ Yes       | Includes CAS from MC68020          |
| MC68060  | ISA60     | ❌ No        | **CAS removed** (Vector 61 trap)   |

## Code Changes

### Modified Files (4 files, +78 -16 lines)

#### 1. `llvm/lib/Target/M68k/M68k.td` (+23 -4 lines)

**Change**: Added `FeatureHardwareCAS` and updated processor definitions

```tablegen
// Lines 59-63: New FeatureHardwareCAS
def FeatureHardwareCAS
  : SubtargetFeature<"hwcas", "HasHardwareCAS", "true",
                     "Target has hardware CAS/CAS2 instructions",
                     [FeatureISA20]>;

// Lines 86-89: Updated processor definitions
def : Proc<"M68020",  [ FeatureISA20, FeatureHardwareCAS ]>;
def : Proc<"M68030",  [ FeatureISA30, FeatureHardwareCAS ]>;
def : Proc<"M68040",  [ FeatureISA40, FeatureHardwareCAS ]>;
def : Proc<"M68060",  [ FeatureISA60 ]>; // NO FeatureHardwareCAS!
```

#### 2. `llvm/lib/Target/M68k/M68kSubtarget.h` (+7 lines)

**Change**: Added member variable and accessor method

```cpp
// Line 65: Member variable declaration
bool HasHardwareCAS = false;

// Lines 97-99: Public accessor method
/// Hardware CAS instruction support (MC68020/030/040 only)
/// Note: MC68060 removed CAS from hardware (Vector 61 trap)
bool hasHardwareCAS() const { return HasHardwareCAS; }
```

**Key Fix**: Added explicit member variable declaration. The M68k backend doesn't use LLVM's auto-generated boolean features pattern - it requires manual declarations.

#### 3. `llvm/lib/Target/M68k/M68kInstrAtomics.td` (+36 -7 lines)

**Change**: Updated predicate from `AtLeastM68020` to `SupportsCAS`

```tablegen
// Line 17: New predicate
def SupportsCAS : Predicate<"Subtarget->hasHardwareCAS()">;

// Lines 29-34: Updated instruction definitions
let Predicates = [SupportsCAS] in {
  def CAS8  : PseudoCAS<M68kCAS8,  i8>;
  def CAS16 : PseudoCAS<M68kCAS16, i16>;
  def CAS32 : PseudoCAS<M68kCAS32, i32>;
}
```

**Added Documentation** (Lines 20-27): Comprehensive hardware specification notes explaining MC68060's CAS removal.

#### 4. `llvm/lib/Target/M68k/M68kISelLowering.cpp` (+12 -4 lines)

**Change**: Updated atomic expansion logic to use `hasHardwareCAS()`

```cpp
// Lines 1881-1894: Updated shouldExpandAtomicCmpXchgInIR()
// MC68000/010: Expand to libcalls (no CAS support)
// MC68020/030/040: Use hardware CAS instructions
// MC68060: Expand to libcalls (CAS removed from hardware)
if (!Subtarget.hasHardwareCAS())
  return AtomicExpansionKind::None; // Fall back to libcalls
```

### New Files (1 file)

#### 5. `llvm/test/CodeGen/M68k/cas-cmpxchg.ll` (new file, 87 lines)

Comprehensive lit test suite covering all atomic widths (8/16/32-bit) with proper FileCheck directives for all CPU variants.

## Build Process

### Issue Encountered: Missing Member Variable Declaration

**Error**: `use of undeclared identifier 'HasHardwareCAS'`

**Root Cause**: M68k backend pattern requires explicit member variable declarations in M68kSubtarget.h. Unlike some LLVM backends that use auto-generated feature booleans, M68k manually declares each feature variable.

**Fix**: Added `bool HasHardwareCAS = false;` at M68kSubtarget.h:65

### Build Timeline

- **LLVM Configure**: ~28 seconds
- **LLVM Build**: ~1832 tasks completed successfully
- **Build Output**: `build/llvm/bin/llc` (52MB, Nov 2 16:48)
- **Installation**: Copied to `toolchain/bin/llc`

## Verification Results

### Code Generation Tests

All 6 M68k CPU variants generate correct code:

| CPU Variant | Expected Output     | Actual Output       | Status    |
|-------------|---------------------|---------------------|-----------|
| M68000      | Libcalls (__sync_*) | Libcalls (__sync_*) | ✅ PASS   |
| M68010      | Libcalls (__sync_*) | Libcalls (__sync_*) | ✅ PASS   |
| M68020      | cas.b/w/l           | cas.b/w/l           | ✅ PASS   |
| M68030      | cas.b/w/l           | cas.b/w/l           | ✅ PASS   |
| M68040      | cas.b/w/l           | cas.b/w/l           | ✅ PASS   |
| M68060      | Libcalls (__sync_*) | Libcalls (__sync_*) | ✅ PASS   |

### Sample Output: MC68060 (Libcalls - Correct)

```assembly
cas8:                                   ; @cas8
    suba.l  #12, %sp
    ; ... setup stack parameters ...
    jsr __sync_val_compare_and_swap_1
    adda.l  #12, %sp
    rts
```

### Sample Output: MC68020 (Hardware CAS - Correct)

```assembly
cas8:                                   ; @cas8
    move.l  (4,%sp), %a0
    move.b  (15,%sp), %d1
    move.b  (11,%sp), %d0
    cas.b %d0, %d1, (%a0)
    rts
```

## Impact

### Correctness

- **MC68060**: Now generates libcalls instead of illegal CAS instructions, preventing Vector 61 traps
- **MC68000/010**: Continues to generate libcalls (correct)
- **MC68020/030/040**: Continues to generate hardware CAS (correct, optimized)

### Performance

- **MC68060**: Same as before (libcalls required due to hardware limitation)
- **MC68020/030/040**: No performance impact (still uses hardware CAS)
- **MC68000/010**: No performance impact (still uses libcalls)

## Related Work

### CAS2 Investigation

Searched for CAS2 (double-word CAS) support in LLVM codebase:
- **Result**: No CAS2 implementation found in M68k backend
- **Reason**: LLVM typically uses `cmpxchg16b` on x86-64 or pair-based approaches
- **Status**: CAS2 remains unimplemented (not required for basic atomic operations)

### Atomic Width Support

Current implementation correctly handles:
- ✅ 8-bit atomics (`i8` / `cas.b`)
- ✅ 16-bit atomics (`i16` / `cas.w`)
- ✅ 32-bit atomics (`i32` / `cas.l`)
- ❌ 64-bit atomics (requires CAS2, unimplemented)

## Next Steps

1. ✅ Verification complete - All tests passing
2. ⏭️ Commit changes to git with descriptive message
3. ⏭️ Update project documentation (m68k-atomic-support-tracker.md)
4. ⏭️ Consider upstream contribution to LLVM project

## References

- **M68k Programmer's Reference Manual**: MC68060 specification (CAS removed)
- **LLVM TableGen**: SubtargetFeature documentation
- **Test Case**: `llvm/test/CodeGen/M68k/cas-cmpxchg.ll`

---

**Implementation**: Claude Code (Opus) working with NeXTRust project  
**Verification**: Manual testing across all 6 M68k CPU variants  
**Build System**: CMake/Ninja with custom LLVM configuration
