!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "DeskBaton" '"$INSTDIR\deskbaton-gui.exe"'
  CreateShortCut "$APPDATA\Microsoft\Windows\SendTo\DeskBaton.lnk" "$INSTDIR\deskbaton-gui.exe" "--send" "$INSTDIR\deskbaton-gui.exe" 0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "DeskBaton"
  Delete "$APPDATA\Microsoft\Windows\SendTo\DeskBaton.lnk"
!macroend
