!macro customInstall
  FileOpen $0 "$INSTDIR\resources\installed-by-nsis" w
  FileClose $0
!macroend

!macro customUnInstall
  Delete "$INSTDIR\resources\installed-by-nsis"
!macroend
