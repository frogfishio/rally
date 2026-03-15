# SPDX-FileCopyrightText: 2026 Alexander R. Croft
# SPDX-License-Identifier: GPL-3.0-or-later

SHELL := /bin/sh

APP_NAME := rally
OS := $(shell uname -s | tr '[:upper:]' '[:lower:]')
ARCH := $(shell uname -m | sed 's/x86_64/amd64/; s/aarch64/arm64/')
DIST_DIR := dist/$(OS)-$(ARCH)
DIST_BIN_DIR := dist/$(OS)-$(ARCH)/bin
DIST_USER_GUIDE := $(DIST_DIR)/USER_GUIDE.md
DIST_README := $(DIST_DIR)/README.md
DIST_LICENSE := $(DIST_DIR)/LICENSE
DIST_EXAMPLE_CONFIG := $(DIST_DIR)/rally.toml.example
RELEASE_BIN := target/release/$(APP_NAME)

.PHONY: bump dist clean distclean

bump:
	@current_version="$$(cat VERSION)"; \
	case "$$current_version" in \
	  ''|*[!0-9.]*) \
	    echo "VERSION must contain only digits and dots" >&2; \
	    exit 1; \
	    ;; \
	esac; \
	new_version="$$(printf '%s\n' "$$current_version" | awk -F. 'BEGIN { OFS = "." } { $$NF += 1; print }')"; \
	awk -v version="$$new_version" 'BEGIN { in_package = 0; updated = 0 } \
	  /^\[package\][[:space:]]*$$/ { in_package = 1; print; next } \
	  /^\[/ && $$0 !~ /^\[package\][[:space:]]*$$/ { in_package = 0 } \
	  in_package && /^version[[:space:]]*=/ && !updated { print "version = \"" version "\""; updated = 1; next } \
	  { print } \
	  END { if (!updated) exit 1 }' Cargo.toml > Cargo.toml.tmp; \
	mv Cargo.toml.tmp Cargo.toml; \
	printf '%s\n' "$$new_version" > VERSION; \
	echo "VERSION $$current_version -> $$new_version"; \
	echo "Cargo.toml package version -> $$new_version"

dist:
	@current_build="$$(cat BUILD)"; \
	case "$$current_build" in \
	  ''|*[!0-9]*) \
	    echo "BUILD must contain only digits" >&2; \
	    exit 1; \
	    ;; \
	esac; \
	new_build="$$((current_build + 1))"; \
	printf '%s\n' "$$new_build" > BUILD; \
	echo "BUILD $$current_build -> $$new_build"
	@cargo build --release
	@mkdir -p "$(DIST_BIN_DIR)"
	@cp "$(RELEASE_BIN)" "$(DIST_BIN_DIR)/$(APP_NAME)"
	@cp "USER_GUIDE.md" "$(DIST_USER_GUIDE)"
	@cp "README.md" "$(DIST_README)"
	@cp "LICENSE" "$(DIST_LICENSE)"
	@cp "rally.toml.example" "$(DIST_EXAMPLE_CONFIG)"
	@echo "Copied $(RELEASE_BIN) to $(DIST_BIN_DIR)/$(APP_NAME)"
	@echo "Copied USER_GUIDE.md to $(DIST_USER_GUIDE)"
	@echo "Copied README.md to $(DIST_README)"
	@echo "Copied LICENSE to $(DIST_LICENSE)"
	@echo "Copied rally.toml.example to $(DIST_EXAMPLE_CONFIG)"

clean:
	@rm -rf target
	@echo "Removed target"

distclean:
	@rm -rf target dist
	@echo "Removed target and dist"