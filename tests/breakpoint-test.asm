INCLUDE "hardware.inc"

SECTION "Header", ROM0[$100]
	jp EntryPoint

	ds $150 - @, 0 ; Make room for the header


SECTION "UnitTests_BreakN", ROM0[$210]
BreakN:
	ld a, 10
loop:
	dec a ; address [0x212]
	jp nz, loop
	ret


SECTION "Main", ROM0
EntryPoint:
	di
	; Shut down audio circuitry
	ld a, 0
	ld [rNR52], a
	call BreakN

Done:
	halt
	jr Done
