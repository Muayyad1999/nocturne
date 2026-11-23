; Hooks for the NSIS installer tauri-bundler writes (`bundle.windows.nsis.installerHooks`).
;
; The uninstaller's "Delete the application data" box removes the folders
; named after this edition's identifier: the WebView's storage and the
; window's size and position. Nocturne's own data is not there. The
; inventory, the encrypted credentials and the key that decrypts them
; (`vault.key`, unless a master password is set) are in %APPDATA%\Nocturne,
; which the other edition and nocturne-server open as well, so an
; uninstaller cannot know it is the last thing using that folder, and does
; not delete it. With the box ticked it says so and where the folder is,
; rather than let the box pass for having deleted them.
;
; `$DeleteAppDataCheckboxState`, `$UpdateMode` and `${PRODUCTNAME}` belong
; to tauri-bundler's template (windows/nsis/installer.nsi), which expands
; this macro in its uninstall section after its own deletions.

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    ${If} ${FileExists} "$APPDATA\Nocturne\*.*"
      MessageBox MB_OK|MB_ICONINFORMATION "${PRODUCTNAME}'s inventory, the credentials and the key that decrypts them are still in$\r$\n$\r$\n$APPDATA\Nocturne$\r$\n$\r$\nThat folder is shared with the other edition of Nocturne and with nocturne-server, so it was left in place. If neither uses it, delete it yourself." /SD IDOK
    ${EndIf}
  ${EndIf}
!macroend
