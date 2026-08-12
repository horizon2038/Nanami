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
ABI_OBJS := \
	$(BUILD_DIR)/debug_call.o \
	$(BUILD_DIR)/ipc_port.o \
	$(BUILD_DIR)/processor.o
NANAMI_OBJ := $(BUILD_DIR)/nanami.o

ABI_LIB := $(BUILD_DIR)/liba9nabi.a
NANAMI_LIB := $(BUILD_DIR)/libnanami.a

.PHONY: all clean FORCE

all: $(OUT)

$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

$(APP_OBJ): $(APP_SRC) | $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $< -o $@

$(START_OBJ): $(SDK_DIR)/arch/$(ARCH)/runtime/start.S | $(BUILD_DIR)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD_DIR)/debug_call.o: $(SDK_DIR)/arch/$(ARCH)/a9n_abi/debug_call.cpp | $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $< -o $@

$(BUILD_DIR)/ipc_port.o: $(SDK_DIR)/arch/$(ARCH)/a9n_abi/ipc_port.cpp | $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $< -o $@

$(BUILD_DIR)/processor.o: $(SDK_DIR)/arch/$(ARCH)/a9n_abi/processor.cpp | $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $< -o $@

$(NANAMI_OBJ): $(SDK_DIR)/cpp/src/nanami/nanami.cpp | $(BUILD_DIR)
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $< -o $@

$(ABI_LIB): $(ABI_OBJS) FORCE
	rm -f $@
	$(AR) rcs $@ $(ABI_OBJS)

$(NANAMI_LIB): $(NANAMI_OBJ) FORCE
	rm -f $@
	$(AR) rcs $@ $(NANAMI_OBJ)

$(OUT): $(START_OBJ) $(APP_OBJ) $(NANAMI_LIB) $(ABI_LIB)
	$(LD) $(LDFLAGS) -o $@ $(START_OBJ) $(APP_OBJ) $(NANAMI_LIB) $(ABI_LIB)

clean:
	rm -rf $(BUILD_DIR)

FORCE:
