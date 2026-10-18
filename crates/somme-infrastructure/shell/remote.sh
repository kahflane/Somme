# Starts the user's login shell on a server with Somme's hooks. Somme wrote
# this folder just before; the hooks remove it once loaded.
d=${0%/*}
read -r SOMME_MARK_TOKEN < "$d/token"
rm -f "$d/token"
export SOMME_MARK_TOKEN SOMME_HOOKS_TMP="$d"

# sshd prints its login messages only when no command starts the shell, so
# show the message of the day here, as it would.
if [ ! -e "$HOME/.hushlogin" ]; then
  if [ -r /run/motd.dynamic ]; then
    cat /run/motd.dynamic
  fi
  if [ -r /etc/motd ] && ! [ /etc/motd -ef /run/motd.dynamic ]; then
    cat /etc/motd
  fi
fi

case ${SHELL##*/} in
  zsh)
    if [ -n "${ZDOTDIR+x}" ]; then
      export SOMME_USER_ZDOTDIR="$ZDOTDIR"
    fi
    export ZDOTDIR="$d" SOMME_SHELL_HOOKS="$d"
    exec "$SHELL" -l
    ;;
  bash)
    exec "$SHELL" --rcfile "$d/somme.bash" -i
    ;;
esac
rm -rf "$d"
exec "$SHELL" -l
