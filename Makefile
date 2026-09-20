PREFIX ?= $(HOME)/.local
BINDIR = $(PREFIX)/bin
DATADIR = $(PREFIX)/share
APPDIR = $(DATADIR)/applications
ICONDIR = $(DATADIR)/icons/hicolor/scalable/apps
UNAME_S := $(shell uname -s)

.PHONY: build install uninstall

build:
	cargo build --release --locked --bin comport

install: build
	# GNU install -D creates parent directories; BSD/macOS install does not.
	mkdir -p "$(BINDIR)"
	install -m 755 target/release/comport "$(BINDIR)/comport"
ifeq ($(UNAME_S),Darwin)
	@echo "Installed $(BINDIR)/comport"
	@echo "A macOS .app bundle lands with packaging; use the binary for now."
else
	mkdir -p "$(ICONDIR)" "$(APPDIR)"
	install -m 644 etc/comport.svg "$(ICONDIR)/comport.svg"
	sed 's|Exec=comport|Exec=$(BINDIR)/comport|' etc/comport.desktop \
		> "$(APPDIR)/comport.desktop"
	chmod 644 "$(APPDIR)/comport.desktop"
	-update-desktop-database "$(APPDIR)" >/dev/null 2>&1
	-gtk-update-icon-cache -f -t "$(DATADIR)/icons/hicolor" >/dev/null 2>&1
	@echo "Installed to $(PREFIX). Make sure $(BINDIR) is in your PATH."
endif

uninstall:
	rm -f "$(BINDIR)/comport"
ifneq ($(UNAME_S),Darwin)
	rm -f "$(APPDIR)/comport.desktop"
	rm -f "$(ICONDIR)/comport.svg"
endif
