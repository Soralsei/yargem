INCLUDE "hardware.inc"

SECTION "Header", ROM0[$100]
	jp EntryPoint

	ds $150 - @, 0 ; Make room for the header


SECTION "UnitTests_ReadNormal", ROM0[$150]
ReadNormal:
	ld a, 123
	ld hl, $C001
	ld [hl], a ; read watchpoint should not run here
	ld b, [hl] ; read watchpoint should be hit here and read 123->123
	ret

SECTION "UnitTests_WriteNormal", ROM0[$170]
WriteNormal:
	ld a, 234
	ld hl, $C002
	ld b, [hl] ; write watchpoint shouldn't run here
	ld [hl], a ; write watchpoint hits here and should read 0->234
	ret

SECTION "UnitTests_ReadPop", ROM0[$190]
ReadPop:
	ld c, $0A
	ld b, $B0
	ld sp, 0xC005
	push bc
	pop de ; read watchpoint hits here : at address 0xC003 (0x0A)
	ret


SECTION "UnitTests_WritePush", ROM0[$210]
	ld c, $0A
	ld b, $B0
	ld sp, 0xC005
	push bc ; write watchpoint hits here: address 0xC004 (0xB0)
	pop de
	ret


SECTION "Main", ROM0
EntryPoint:
	di
	; Shut down audio circuitry
	ld a, 0
	ld [rNR52], a
	call ReadNormal
	call WriteNormal
	call ReadPop

Done:
	halt
	jr Done
