`gcc16_lto_aarch64.elf`: `gcc -g -O2 -flto -o lto.elf lto_a.c lto_b.c` with GCC
16.2.0 (the `gcc:latest` image, AArch64 Linux). GCC's LTO writes the concrete
`scale` subprogram and its parameters in one compilation unit and their names
and types in another, joined by `DW_FORM_ref_addr`. A reader that follows only
unit-local references reads every type of `scale` as void (DecBench #120).
