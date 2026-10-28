ASM  = rgbasm
LINK = rgblink
FIX  = rgbfix

# Find all .asm files in the directory
SRCS = $(wildcard *.asm)

# Convert the list of .asm files into .gb targets (e.g., test1.asm -> test1.gb)
ROMS = $(SRCS:.asm=.gb)

# 'make' or 'make all' builds every ROM
all: $(ROMS)

# Pattern rule: How to create ANY .gb file from its .o file
%.gb: %.o
	$(LINK) -o $@ $<
	$(FIX) -v -p 0xFF $@

# Pattern rule: How to assemble ANY .asm file into a .o file
%.o: %.asm
	$(ASM) -o $@ $<

clean:
	rm -f *.gb *.o

.PHONY: all clean
