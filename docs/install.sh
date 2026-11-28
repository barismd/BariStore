# BariStore for rootshell. In a rootshell tab (its sh runs a script given with -c,
# not one piped to it; rootshell's shell runs no pipeline of programs either):
#   sh -c "$(curl -fsSL https://raw.githubusercontent.com/barismd/BariStore/test/docs/install.sh)"
# Puts the newest BariStore in ~/bin/baristore.wasm (the newest release, or the
# newest test build while no release has one), the baristore command in
# ~/.rootshellrc, and opens the store in Safari. Run it again to update.
cd ~ || exit 1
# rootshell holds this tab's curl and mkdir while BariStore runs in another tab:
# then only the command is set up.
if [ -f .baristore/live ] && [ -f bin/baristore.wasm ]; then
  if bin/baristore.wasm --join install; then exit 0; fi
fi
[ -d bin ] || mkdir bin
[ -d .baristore ] || mkdir .baristore
echo "Downloading BariStore..."
if curl -fsL -o bin/baristore.new https://github.com/barismd/BariStore/releases/latest/download/baristore.wasm; then
  :
else
  # GitHub lists releases by version, newest first.
  curl -fsSL -o .baristore/releases.json "https://api.github.com/repos/barismd/BariStore/releases?per_page=30" || exit 1
  release=$(grep -o -m 1 'download/v[0-9A-Za-z.-]*/baristore.wasm' .baristore/releases.json)
  rm -f .baristore/releases.json
  if [ -z "$release" ]; then
    echo "No BariStore build was found on GitHub. Try again later."
    exit 1
  fi
  curl -fsSL -o bin/baristore.new "https://github.com/barismd/BariStore/releases/$release" || exit 1
fi
mv -f bin/baristore.new bin/baristore.wasm || exit 1
bin/baristore.wasm install || exit 1
. ./.rootshellrc
baristore
