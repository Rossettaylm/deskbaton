!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "DeskBaton" '"$INSTDIR\DeskBaton.exe"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "DeskBaton"
!macroend
