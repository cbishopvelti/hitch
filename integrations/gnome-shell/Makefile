UUID = hitch@cbishopvelti.github.io
DEST = $(HOME)/.local/share/gnome-shell/extensions/$(UUID)
FILES = metadata.json extension.js

.PHONY: install uninstall pack test

install:
	mkdir -p $(DEST)
	cd $(UUID) && cp $(FILES) $(DEST)/

uninstall:
	rm -rf $(DEST)

# The same zip `gnome-extensions pack` makes (it crashes on GNOME Shell 51.0
# in a container, so build it directly).
pack:
	mkdir -p dist
	cd $(UUID) && python3 -m zipfile -c ../dist/$(UUID).shell-extension.zip $(FILES)

test:
	test/run-headless.sh
