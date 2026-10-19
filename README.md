# ScreenTint

Aplicativo de bandeja para aplicar um filtro de cor à tela, reduzir o brilho por software e ajustar o
brilho dos monitores por DDC/CI. Escrito em Rust, interface em [Slint](https://slint.dev).

## O que faz

- Aplica um multiplicador por canal (R, G, B) sobre toda a tela, em todos os monitores.
  A cor do filtro vem de uma temperatura em Kelvin (1200–6500 K) ou de uma cor escolhida por matiz e saturação.
- Presets: Quente (3600 K), Noite (2300 K, brilho 75%) e Escuro (sem cor, brilho 40%).
- Controles de intensidade do filtro (0–100%) e de brilho da tela (10–100%, escurecimento por software).
- Ajusta o brilho físico dos monitores (DDC/CI, VCP 0x10) e de painéis de notebook.
  Com "Vincular" ligado, um slider mestre move todos os monitores preservando a diferença entre eles.
  Desligado, cada monitor tem seu slider.
- Agendamento por horário (início e fim, aceita intervalo que cruza a meia-noite).
- Transição suave entre cores (interpolação exponencial, ~16 ms por passo).
- "Fixar painel": mantém o painel aberto quando perde o foco. A opção é salva e o painel abre ao iniciar o app.
- Atalhos globais: `Ctrl+Alt+T` liga/desliga, `Ctrl+Alt+↑` e `Ctrl+Alt+↓` ajustam o brilho em passos de 5%.
- Inicia com o sistema (opcional) e permite apenas uma instância: abrir de novo mostra o painel da instância existente.

## Backends

| Sistema | Tom da tela | Brilho do monitor |
|---------|-------------|-------------------|
| Windows | `MagSetFullscreenColorEffect` (Magnification API) | `dxva2` (DDC/CI) e `\.\LCD` (painel de notebook) |
| macOS | `CGSetDisplayTransferByFormula` | `DisplayServices` (tela interna, carregado em runtime) e [`m1ddc`](https://github.com/waydabber/m1ddc) (externos) |
| Linux (X11) | rampas de gamma do RandR | `/sys/class/backlight` e [`ddcutil`](https://www.ddcutil.com/) |

Limitações:

- Em sessões Wayland sem XWayland não há API para gamma, então o tom não é aplicado.
- Monitores sem DDC/CI, ou com DDC/CI desativado no menu do monitor, não aparecem na seção de brilho.
- As escritas DDC/CI levam de 50 a 200 ms. Elas rodam em uma thread separada e são agrupadas:
  só o último valor pedido para cada monitor é enviado.

## Implementação

- O multiplicador final por canal é `cor_do_filtro * intensidade * brilho`, onde a cor do filtro é interpolada
  entre branco e a cor base conforme a intensidade. A temperatura é convertida para RGB com a aproximação de Tanner Helland.
- A interface usa o renderizador por software do Slint com janela sem moldura e transparente. A fonte Inter
  é embutida no binário.
- O ícone da bandeja é gerado em tempo de execução (um disco com um recorte, na cor atual do filtro).

## Configuração

Arquivo `settings.ini` (formato `chave=valor`) em:

- Windows: `%APPDATA%\ScreenTint`
- macOS: `~/Library/Application Support/ScreenTint`
- Linux: `~/.config/screentint`

Início com o sistema: chave `Run` do registro (Windows), LaunchAgent (macOS) ou entrada em
`~/.config/autostart` (Linux).

## Instalação

Os pacotes são gerados pelo workflow `.github/workflows/release.yml` e publicados na página de Releases a cada tag `vX.Y.Z`.

| Sistema | Arquivo | Observações |
|---------|---------|-------------|
| Windows | `ScreenTint-X.Y.Z-windows-x64-setup.exe` | Instalação por usuário (sem admin) em `%LOCALAPPDATA%\Programs`, atalho no Menu Iniciar, desinstalador e opção de iniciar com o Windows. Também há `...-portable.zip` com só o `.exe`. |
| macOS | `ScreenTint-X.Y.Z-macos-universal.dmg` | `.app` universal (Apple Silicon e Intel), sem ícone no Dock (`LSUIElement`). |
| Linux | `ScreenTint-X.Y.Z-linux-amd64.deb` ou `...-linux-x86_64.tar.gz` | O `.deb` recomenda `ddcutil`. O `.tar.gz` traz um `install.sh` que instala em `~/.local`. |

Os binários não são assinados:

- Windows: o SmartScreen pode avisar ("editor desconhecido"). Antivírus com análise comportamental podem sinalizar
  um executável não assinado que registra início automático.
- Cada Release inclui `SHA256SUMS.txt` para conferir os arquivos baixados. Um falso positivo pode ser
  reportado à Microsoft em <https://www.microsoft.com/wdsi/filesubmission>.
- macOS: a assinatura é ad hoc, sem Developer ID nem notarização. Na primeira abertura, use clique direito > Abrir,
  ou `xattr -dr com.apple.quarantine /Applications/ScreenTint.app`.

O instalador do Windows foi testado (instalação, desinstalação e remoção da chave de início automático).
Os pacotes de macOS e Linux são gerados no CI e não foram testados em hardware.

## Compilar

Requer Rust estável.

```sh
cargo build --release
```

O binário fica em `target/release/ScreenTint` (`ScreenTint.exe` no Windows).

| Sistema | Dependências |
|---------|--------------|
| Windows | Build Tools do Visual Studio com a carga de trabalho C++ e o Windows SDK. Compilar pelo PowerShell ou cmd: no Git Bash o `link` do coreutils sobrepõe o `link.exe` do MSVC. |
| macOS | Xcode Command Line Tools. |
| Linux | `pkg-config`, `libfontconfig1-dev`, `libxkbcommon-dev`, `libx11-dev` (nomes de pacote do Debian/Ubuntu). Opcional: `ddcutil`. |

## Estrutura

```
src/main.rs        UI, bandeja, atalhos, animação, thread de brilho
src/model.rs       configurações, conversão de cor, agenda, persistência
src/platform/      windows.rs, macos.rs, linux.rs
ui/app.slint       painel e menu da bandeja
assets/            logo, ícone e pin
```

## Licença

MIT ([LICENSE](LICENSE)). A fonte Inter está sob a SIL Open Font License ([ui/fonts/LICENSE.txt](ui/fonts/LICENSE.txt)).
