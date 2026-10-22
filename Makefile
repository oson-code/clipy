# Build, install and package Clipy for Linux.
#   make install            build and install to $(PREFIX) (use sudo for /usr/local)
#   make install-extension  install the GNOME Shell extension for the current user
#   make dist               build the .deb and .tar.gz to share, into dist/ (see INSTALL.md)

PREFIX ?= /usr/local
DESTDIR ?=
BIN := target/release/clipy
APP_ID := uz.osoncode.Clipy
UUID := clipy@osoncode.uz
EXT_SRC := gnome-extension/$(UUID)
EXT_USER_DIR := $(HOME)/.local/share/gnome-shell/extensions/$(UUID)
ICON_SIZES := 16 32 128 256 512

.PHONY: build install uninstall install-extension uninstall-extension dist

build:
	cargo build --release

install:
	@test -x $(BIN) || { echo "Run 'make build' first (as your user, not root)."; exit 1; }
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/clipy
	install -Dm644 resources/$(APP_ID).desktop $(DESTDIR)$(PREFIX)/share/applications/$(APP_ID).desktop
	for s in $(ICON_SIZES); do \
		install -Dm644 resources/icons/app_$$s.png $(DESTDIR)$(PREFIX)/share/icons/hicolor/$${s}x$${s}/apps/clipy.png; \
	done

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/clipy $(DESTDIR)$(PREFIX)/share/applications/$(APP_ID).desktop
	for s in $(ICON_SIZES); do rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/$${s}x$${s}/apps/clipy.png; done

install-extension:
	mkdir -p $(EXT_USER_DIR)
	cp $(EXT_SRC)/*.js $(EXT_SRC)/metadata.json $(EXT_SRC)/stylesheet.css $(EXT_USER_DIR)/
	@if gnome-extensions enable $(UUID) 2>/dev/null; then \
		echo "Enabled $(UUID)."; \
	else \
		echo "Installed. GNOME on Wayland loads new extensions after you log out and back in;"; \
		echo "then run: gnome-extensions enable $(UUID)"; \
	fi

uninstall-extension:
	-gnome-extensions disable $(UUID)
	rm -rf $(EXT_USER_DIR)

dist: build
	scripts/package.sh
