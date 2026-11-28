# BariStore on the Home Screen

One tap starts BariStore and opens it in Safari. Set it up once.

## 1. A rootshell profile

1. In rootshell, open **Profiles** and tap **+**.
2. **Protocol**: Local Shell. **Name**: BariStore.
3. **Startup Command**: `baristore`
4. Save.

## 2. A Shortcut

1. Open **Shortcuts**, tap **+**.
2. Add the action **Open Connection Profile** (rootshell) and pick **BariStore**.
3. Name the Shortcut **BariStore**.
4. Tap the Shortcut's **ⓘ** › **Add to Home Screen**.

## How it behaves

- Tap the icon: rootshell opens a tab, BariStore starts there and Safari shows the store.
- Tap it while BariStore runs: Safari shows the store again. No second BariStore starts.
- rootshell opens a new tab every time a profile runs, and only you can close a tab. When BariStore was already
  running, the new tab only opened Safari: close it. BariStore cannot close tabs, and rootshell has no way yet to
  reuse a profile's tab.
- BariStore quits by itself after 3 minutes without use, or with **⏻** on its page. Its tab then shows a prompt and
  can be closed too.

Safari opens by itself only with **rootshell › Settings › Terminal › Open Links from Programs** on. Without it,
touch and hold **Open BariStore** in the tab and tap **Open Link**.
