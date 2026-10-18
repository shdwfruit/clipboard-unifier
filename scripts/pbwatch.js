#!/usr/bin/osascript -l JavaScript
// Passive macOS pasteboard watcher. Prints changeCount and type names on every change
// and flags items that arrived via Universal Clipboard. Never reads clipboard contents.
//
//   osascript -l JavaScript scripts/pbwatch.js
//
// Then copy something on your iPhone: a line with remote=true should appear here
// within a second or two.

ObjC.import('AppKit');

const REMOTE_MARKER = 'com.apple.is-remote-clipboard';

function run() {
  const pb = $.NSPasteboard.generalPasteboard;
  let last = -1;
  console.log(`watching general pasteboard (accessBehavior=${pb.accessBehavior}); Ctrl-C to stop`);
  for (;;) {
    const changeCount = pb.changeCount;
    if (changeCount !== last) {
      last = changeCount;
      const types = ObjC.deepUnwrap(pb.types) || [];
      const remote = types.includes(REMOTE_MARKER);
      console.log(`${new Date().toISOString()} changeCount=${changeCount} remote=${remote} types=[${types.join(', ')}]`);
    }
    delay(0.25);
  }
}
