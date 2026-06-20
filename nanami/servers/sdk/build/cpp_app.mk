CXX := clang++
CC := clang
AR := llvm-ar
LD := ld.lld

SDK_DIR ?= ../../sdk
APP_DIR ?= .
BUILD_DIR ?= $(APP_DIR)/build
OUT ?= $(BUILD_DIR)/app.elf
ARCH ?= x86_64

TARGET_TRIPLE_x86_64 := x86_64-unknown-elf
TARGET_TRIPLE ?= $(TARGET_TRIPLE_$(ARCH))

CXXFLAGS += -O2 -Wall -Wextra -std=c++20 -ffreestanding -fno-exceptions -fno-rtti -fno-stack-protector -mno-red-zone -nostdlib --target=$(TARGET_TRIPLE)
CFLAGS += -O2 -Wall -Wextra -ffreestanding -fno-stack-protector -mno-red-zone -nostdlib --target=$(TARGET_TRIPLE)
LDFLAGS += -static -no-pie -T $(SDK_DIR)/arch/$(ARCH)/linker/nanami_user.ld

INCLUDES += -I$(SDK_DIR)/cpp/include

APP_SRC ?= $(APP_DIR)/src/main.cpp
APP_OBJ := $(BUILD_DIR)/app_main.o
START_OBJ := $(BUILD_DIR)/start.o
ABI_OBJ := $(BUILD_DIR)/debug_call.o
NANAMI_OBJ := $(BUILD_DIR)/nanami.o

ABI_LIB := $(BUILD_DIR)/liba9nabi.a
NANAMI_LIB := $(BUILD_DIR)/libnanami.a

.PHONY: all clean

all: $(OUT)

$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

$(APP_OBJ): $(APP_SRC) | $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $< -o $@

$(START_OBJ): $(SDK_DIR)/arch/$(ARCH)/runtime/start.S | $(BUILD_DIR)
	$(CC) $(CFLAGS) -c $< -o $@

$(ABI_OBJ): $(SDK_DIR)/arch/$(ARCH)/a9n_abi/debug_call.cpp | $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $< -o $@

$(NANAMI_OBJ): $(SDK_DIR)/cpp/src/nanami/nanami.cpp | $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $< -o $@

$(ABI_LIB): $(ABI_OBJ)
	$(AR) rcs $@ $^

$(NANAMI_LIB): $(NANAMI_OBJ)
	$(AR) rcs $@ $^

$(OUT): $(START_OBJ) $(APP_OBJ) $(NANAMI_LIB) $(ABI_LIB)
	$(LD) $(LDFLAGS) -o $@ $(START_OBJ) $(APP_OBJ) $(NANAMI_LIB) $(ABI_LIB)

clean:
	rm -rf $(BUILD_DIR)
