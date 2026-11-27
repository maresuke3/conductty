# conductty のシェル連携: プロンプトの区切り (OSC 133)、カレントディレクトリ (OSC 7)、タイトル (OSC 2) を送る
'builtin' 'autoload' '-Uz' 'add-zsh-hook'

_conductty_precmd() {
  'builtin' 'local' ret=$?
  # 前のコマンドが終わったことを、終了コード付きで知らせる
  if [[ -n "$_conductty_running" ]]; then
    'builtin' 'printf' '\e]133;D;%s\a' "$ret"
    _conductty_running=
  fi
  # カレントディレクトリ（% は %25 にする）
  'builtin' 'printf' '\e]7;file://%s%s\a' "$HOST" "${PWD//\%/%25}"
  # タイトルはカレントディレクトリ
  'builtin' 'print' -Pn '\e]2;%~\a'
  # プロンプトの始まり
  'builtin' 'printf' '\e]133;A\a'
  # ウィンドウの大きさを変えたときなどにプロンプトが描き直されても印が付くよう、PS1 の前後にも入れる
  # NOTE: あとで動くテーマが PS1 を作り直すと消えるが、上の printf でプロンプトの始まりは分かる
  if [[ "$PS1" != *$'\e]133;B'* ]]; then
    PS1=$'%{\e]133;A\a%}'"$PS1"$'%{\e]133;B\a%}'
  fi
}

_conductty_preexec() {
  # コマンドの出力の始まり
  'builtin' 'printf' '\e]133;C\a'
  _conductty_running=1
  # タイトルは実行するコマンド（制御文字は取り除く）
  'builtin' 'print' -rn -- $'\e]2;'"${1//[[:cntrl:]]}"$'\a'
}

add-zsh-hook precmd _conductty_precmd
add-zsh-hook preexec _conductty_preexec
