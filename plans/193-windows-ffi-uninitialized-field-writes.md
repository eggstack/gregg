# Plan 193: Replace uninitialized-field writes in the Windows collector FFI

Status: proposed (not started).

Source: 2026-11-17 security audit, finding 9.

Depends on: current post-186 `main`. Independent of Plans 091 and 183-192.

## Problem

Three Windows collector sites initialize a struct field through a place
expression on `MaybeUninit`:

```rust
let mut mem_info = MaybeUninit::<ffi::MemoryStatusEx>::uninit();
(*mem_info.as_mut_ptr()).dw_length = std::mem::size_of::<...>() as u32;
```

Locations:

- `crates/gregg-host/src/windows/source.rs:1104` (`physical_memory`,
  `GlobalMemoryStatusEx`, `dw_length`),
- `crates/gregg-host/src/windows/source.rs:1152` (`commit`,
  `GetPerformanceInfo`, `cb`; buffer is `zeroed()` but the reference form is
  still the wrong idiom),
- `crates/gregg-host/src/windows/source.rs:1492` (`get_os_info`,
  `RtlGetVersion`, `dw_os_version_info_size`).

Dereferencing `*as_mut_ptr()` to form a place creates a reference to a `T` that
is not yet initialized, which is undefined behavior per the `MaybeUninit`
contract even when the struct is POD. The API calls themselves are used
correctly: each return/status is checked before `assume_init`, and no field is
read after a failure. This is a formal-UB and idiom defect, not an observed
miscompilation, and there is no attacker-controlled input in the path — the
values come from fixed sizes and kernel/OS calls.

The same search (`(*.*as_mut_ptr()).field =`) across the workspace finds no
other site.

## Scope

In scope: the three writes, SAFETY comments, and a source-level check that the
pattern does not return. Out of scope: any change to the Windows APIs used,
the sizes, the return-code handling, or the crate's unsafe allowlist entries
(the file is already allowlisted; no new unsafe block is introduced beyond the
existing ones).

## Implementation

1. At each site, replace the place expression with a raw-pointer write:

   ```rust
   std::ptr::addr_of_mut!((*mem_info.as_mut_ptr()).dw_length)
       .write(u32::try_from(std::mem::size_of::<ffi::MemoryStatusEx>()).unwrap_or(u32::MAX));
   ```

   or, where the code already uses `zeroed()` and the type is POD, keep
   `zeroed()` and use the same `addr_of_mut!` form for consistency.

2. Add one SAFETY comment per site stating: the pointer comes from a live
   `MaybeUninit`, the write targets a field inside that allocation, the field
   is the API-required size/length member, and the call's success is checked
   before `assume_init`.

3. Do not change the `MaybeUninit::uninit()` vs `zeroed()` choice at the
   `GetPerformanceInfo` site; the existing comment explains why zeroed is
   used there.

4. Add a comment in the module (or a `#[cfg(test)]` source-scan test) that the
   `(*…as_mut_ptr()).field =` form is forbidden in this file. A source-scan
   test is cheap and visible, but it must not match its own assertion text:
   build the needle at runtime from parts (or stop the scan before
   `#[cfg(test)]`).

   ```rust
   #[test]
   fn windows_ffi_does_not_write_fields_through_place_expressions() {
       let source = include_str!("source.rs");
       // Built from parts so this assertion does not match its own needle.
       let needle = ["as_mut_ptr", "())."].concat();
       assert!(!source.contains(&needle));
   }
   ```

   If the exact text is too broad (it also matches legitimate
   `(*table.cast::<u32>())` reads), narrow the needle to a field prefix
   (`as_mut_ptr` + `()).dw_`, `()).cb`) or scan for the dereference form and
   whitelist the known reads. Keep the runtime-built needle in every variant.

## Tests

- The source-scan unit test above (runs on every platform because
  `include_str!` does not compile the Windows FFI).
- Existing Windows collector tests keep passing on the native Windows job;
  behavior is unchanged.
- `cargo check -p gregg-host --all-features --target x86_64-pc-windows-gnu`
  compiles locally if the target is installed; otherwise the native Windows CI
  job is the authority.

## Acceptance criteria

- [x] The three sites use `addr_of_mut!` (or an equivalent raw write) and have
      a SAFETY comment.
- [x] No `(*…as_mut_ptr()).field =` remains in `gregg-host/src/windows`.
- [x] The Windows collector's values, return-code handling, and struct sizes
      are unchanged.
- [ ] `cargo fmt`, Clippy (including the native Windows Clippy job), the
      native Windows tests, and `./scripts/check-local.sh` are clean.

## Verification

```bash
cargo test -p gregg-host --all-features
cargo check -p gregg-host --all-features --target x86_64-pc-windows-gnu
./scripts/check-local.sh
```

The native Windows CI job is the platform authority; a green local
cross-check is not closure evidence on its own.

## Risks

- The change is mechanical and behavior-preserving. The only risk is a
  mis-typed field name or size cast, which the compiler catches on the Windows
  target.
- There is no runtime test that can prove the absence of UB; the plan relies
  on the documented `MaybeUninit` contract and the compile check, and says so
  rather than claiming dynamic verification.

## Closure record

### What shipped

- The three sites — `physical_memory` (`dw_length`), `commit` (`cb`), and
  `get_os_info` (`dw_os_version_info_size`) — write their API-required size member
  with `std::ptr::addr_of_mut!((*buf.as_mut_ptr()).field).write(value)`, each with
  a SAFETY comment stating the live `MaybeUninit`, the in-allocation field
  projection, the API role of the field, and that success is checked before
  `assume_init`.
- Nothing else changed: the same APIs, size expressions, casts, return-code
  handling, and read paths. `MaybeUninit::zeroed()` at the `GetPerformanceInfo`
  site and its explanatory comment stand.
- `crates/gregg-host/src/lib.rs` gained a non-platform-gated source-scan test that
  reaches `windows/source.rs` with `include_str!`, builds its needle from parts so
  it cannot match itself, and includes a positive control so it cannot silently
  become vacuous.

### Verification

- `cargo test -p gregg-host --all-features` — 64 passed;
  `tests::windows_ffi_does_not_write_fields_through_place_expressions` runs on
  macOS and passes.
- `cargo check -p gregg-host --all-features --target x86_64-pc-windows-gnu` —
  succeeded, so the edited Windows FFI compiles for a Windows target.
- `cargo fmt -p gregg-host -- --check` — clean; the changed file is Clippy-clean
  (one `explicit_auto_deref` finding in the new test was fixed).
- `grep -c 'as_mut_ptr())\.' crates/gregg-host/src/windows/source.rs` is 3, all
  three inside `addr_of_mut!`.

### Not verified here

- Native Windows MSVC compilation, the native Windows Clippy/test job, and any
  runtime behavior. The cross-compile is an aid; the Windows CI job is the
  authority. No dynamic test can prove the absence of UB — the change rests on the
  documented `MaybeUninit` contract plus the compile check and the source scan.
