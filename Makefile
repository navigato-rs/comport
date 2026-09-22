PREFIX ?= $(HOME)/.local
BINDIR = $(PREFIX)/bin
DATADIR = $(PREFIX)/share
APPDIR = $(DATADIR)/applications
ICONDIR = $(DATADIR)/icons/hicolor/scalable/apps
ICONPNGDIR = $(DATADIR)/icons/hicolor/256x256/apps
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
	mkdir -p "$(ICONDIR)" "$(ICONPNGDIR)" "$(APPDIR)"
	install -m 644 etc/comport.svg "$(ICONDIR)/comport.svg"
	install -m 644 etc/comport.png "$(ICONPNGDIR)/comport.png"
	sed 's|Exec=comport|Exec=$(BINDIR)/comport|' etc/comport.desktop \
		> "$(APPDIR)/comport.desktop"
	chmod 644 "$(APPDIR)/comport.desktop"
	-update-desktop-database "$(APPDIR)" >/dev/null 2>&1 || true
	-gtk-update-icon-cache -f -t "$(DATADIR)/icons/hicolor" >/dev/null 2>&1 || true
	@mimeapps="$${XDG_CONFIG_HOME:-$(HOME)/.config}/mimeapps.list"; \
	mkdir -p "$$(dirname "$$mimeapps")"; \
	touch "$$mimeapps"; \
	if ! grep -q '^\[Default Applications\]' "$$mimeapps"; then \
		printf '\n[Default Applications]\n' >> "$$mimeapps"; \
	fi; \
	if ! grep -q '^x-scheme-handler/comport=' "$$mimeapps"; then \
		printf 'x-scheme-handler/comport=comport.desktop\n' >> "$$mimeapps"; \
	fi; \
	others=$$(grep -l 'x-scheme-handler/mattermost' /usr/share/applications/*.desktop "$$HOME/.local/share/applications"/*.desktop 2>/dev/null | grep -v '/comport.desktop$$' || true); \
	if grep -q '^x-scheme-handler/mattermost=' "$$mimeapps"; then \
		current=$$(grep '^x-scheme-handler/mattermost=' "$$mimeapps" | head -n 1); \
		echo "Left mattermost:// as $$current."; \
	elif [ -n "$$others" ]; then \
		echo "Left mattermost:// with the already installed handler."; \
	else \
		printf 'x-scheme-handler/mattermost=comport.desktop\n' >> "$$mimeapps"; \
		echo "Registered ComPort for mattermost:// (nothing else claimed it)."; \
	fi
	@echo "Installed to $(PREFIX). Make sure $(BINDIR) is in your PATH."
endif

uninstall:
	rm -f "$(BINDIR)/comport"
ifneq ($(UNAME_S),Darwin)
	rm -f "$(APPDIR)/comport.desktop"
	rm -f "$(ICONDIR)/comport.svg"
	rm -f "$(ICONPNGDIR)/comport.png"
endif
