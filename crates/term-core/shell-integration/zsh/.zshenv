# conductty のシェル連携（Ghostty と同じやり方）
# zsh はまず ZDOTDIR の .zshenv を読むので、ここで ZDOTDIR をユーザーのものに戻し、ユーザーの .zshenv を読んでから、
# 対話シェルならシェル連携を読み込む。このあと読まれる .zprofile / .zshrc はユーザーのもの
if [[ -n "${CONDUCTTY_ZSH_ZDOTDIR+X}" ]]; then
  'builtin' 'export' ZDOTDIR="$CONDUCTTY_ZSH_ZDOTDIR"
  'builtin' 'unset' 'CONDUCTTY_ZSH_ZDOTDIR'
else
  'builtin' 'unset' 'ZDOTDIR'
fi

{
  'builtin' 'typeset' _conductty_file="${ZDOTDIR-$HOME}/.zshenv"
  [[ ! -r "$_conductty_file" ]] || 'builtin' 'source' '--' "$_conductty_file"
} always {
  if [[ -o 'interactive' && -r "${CONDUCTTY_SHELL_INTEGRATION_DIR}/conductty-integration.zsh" ]]; then
    'builtin' 'source' '--' "${CONDUCTTY_SHELL_INTEGRATION_DIR}/conductty-integration.zsh"
  fi
  'builtin' 'unset' '_conductty_file'
}
