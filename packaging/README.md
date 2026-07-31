# Registering `.lil`

A `.lil` **is a typst file**. The extension exists for one reason: so that
double-clicking a figure opens lilook. lilook cannot register `.typ` without
taking every typst file away from whatever editor the user already uses, and
that would be rude.

Nothing lilook-only is ever written into a `.lil` — no header, no version
marker, no metadata. `typst compile` reads one directly, and any text editor
shows plain typst. If your editor highlights `.typ`, one line teaches it this
extension too:

```jsonc
// VS Code settings.json
"files.associations": { "*.lil": "typst" }
```

```lua
-- Neovim
vim.filetype.add({ extension = { lil = "typst" } })
```

## Linux

```sh
install -Dm644 lilook-mime.xml ~/.local/share/mime/packages/lilook.xml
install -Dm644 lilook.desktop  ~/.local/share/applications/lilook.desktop
update-mime-database ~/.local/share/mime
update-desktop-database ~/.local/share/applications
```

## macOS

The `Info.plist` fragment below goes in the app bundle. `LSHandlerRank` is
`Alternate` on purpose: lilook is *an* editor for these files, not the only one.

```xml
<key>CFBundleDocumentTypes</key>
<array>
  <dict>
    <key>CFBundleTypeName</key>            <string>lilaq figure</string>
    <key>CFBundleTypeExtensions</key>      <array><string>lil</string></array>
    <key>CFBundleTypeRole</key>            <string>Editor</string>
    <key>LSHandlerRank</key>               <string>Alternate</string>
    <key>LSItemContentTypes</key>          <array><string>org.lilaq.figure</string></array>
  </dict>
</array>
```
