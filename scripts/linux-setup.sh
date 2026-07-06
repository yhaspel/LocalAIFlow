#!/usr/bin/env bash
# Local AI Flow — Linux one-time setup for the optional capabilities.
# Everything here is also explained by `local-ai-flow --doctor`.
set -euo pipefail

echo "Local AI Flow — Linux setup"
echo "This configures the OPTIONAL pieces: ydotool/uinput for Wayland typing,"
echo "the input group for evdev hotkey fallback, and espeak-ng for Kokoro TTS."
echo

if command -v apt-get >/dev/null; then
  PKG="sudo apt-get install -y"
  SUGGEST="espeak-ng libespeak-ng1 ydotool wtype xdotool speech-dispatcher"
elif command -v dnf >/dev/null; then
  PKG="sudo dnf install -y"
  SUGGEST="espeak-ng ydotool wtype xdotool speech-dispatcher"
elif command -v pacman >/dev/null; then
  PKG="sudo pacman -S --needed"
  SUGGEST="espeak-ng ydotool wtype xdotool speech-dispatcher"
else
  PKG="<your package manager>"
  SUGGEST="espeak-ng ydotool wtype xdotool speech-dispatcher"
fi

echo "1) Recommended packages:"
echo "   $PKG $SUGGEST"
read -r -p "   Install now? [y/N] " yn
[[ "${yn,,}" == "y" ]] && $PKG $SUGGEST || true

echo
echo "2) udev rule for /dev/uinput (needed by ydotoold):"
RULES_SRC="$(cd "$(dirname "$0")/../packaging" && pwd)/99-localaiflow-uinput.rules"
read -r -p "   Install $RULES_SRC to /etc/udev/rules.d/? [y/N] " yn
if [[ "${yn,,}" == "y" ]]; then
  sudo cp "$RULES_SRC" /etc/udev/rules.d/
  sudo udevadm control --reload
  sudo udevadm trigger
fi

echo
echo "3) 'input' group membership (evdev hotkey fallback + uinput access):"
if id -nG "$USER" | grep -qw input; then
  echo "   already a member ✓"
else
  read -r -p "   Add $USER to the input group? [y/N] " yn
  [[ "${yn,,}" == "y" ]] && sudo usermod -aG input "$USER" && echo "   log out and back in to take effect"
fi

echo
echo "4) ydotool daemon (Wayland typing on GNOME/KDE):"
if systemctl --user list-unit-files 2>/dev/null | grep -q ydotool; then
  read -r -p "   Enable ydotoold user service now? [y/N] " yn
  [[ "${yn,,}" == "y" ]] && systemctl --user enable --now ydotool
else
  echo "   no ydotool user service found — your distro may ship one as 'ydotool.service'"
fi

echo
echo "Done. Verify with:  local-ai-flow --doctor"
