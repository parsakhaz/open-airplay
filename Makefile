.PHONY: all build open-airplay doubletake-ctl doubletake-test-receiver release install uninstall test clean

PREFIX ?= /usr/local
all: open-airplay doubletake-ctl doubletake-test-receiver

build: all

open-airplay:
	go build -o bin/open-airplay ./cmd/open-airplay

doubletake-ctl:
	go build -o bin/doubletake-ctl ./cmd/doubletake-ctl

doubletake-test-receiver:
	go build -o bin/doubletake-test-receiver ./cmd/doubletake-test-receiver

release:
	CGO_ENABLED=0 go build -ldflags='-s -w -extldflags=-static' -o open-airplay ./cmd/open-airplay

test:
	go test ./...

install: all
	install -m 755 bin/open-airplay $(PREFIX)/bin/
	install -m 755 bin/doubletake-ctl $(PREFIX)/bin/
	install -m 755 bin/doubletake-test-receiver $(PREFIX)/bin/

uninstall:
	rm -f $(PREFIX)/bin/open-airplay
	rm -f $(PREFIX)/bin/doubletake-ctl
	rm -f $(PREFIX)/bin/doubletake-test-receiver

clean:
	rm -rf bin/
	go clean -testcache
