; Space Studio — Windows 安装器（NSIS）
; 深空星球主题图标；产物 SpaceStudio_Setup.exe
; 用法：makensis -DSRCROOT=C:\path\to\space-os installer.nsi
Unicode True
RequestExecutionLevel admin

!ifndef SRCROOT
  !define SRCROOT "."
!endif

!define APP_NAME "Space Studio"
!define APP_EXE "SpaceStudio.exe"
!define APP_VERSION "0.1.0"

Name "${APP_NAME}"
OutFile "dist\SpaceStudio_${APP_VERSION}_setup.exe"
InstallDir "$PROGRAMFILES64\SpaceStudio"
InstallDirRegKey HKLM "Software\SpaceStudio" "InstallDir"
SetCompressor /SOLID lzma

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Install"
  SetOutPath "$INSTDIR"
  File "${SRCROOT}\space-studio\src-tauri\target\x86_64-pc-windows-gnu\release\${APP_EXE}"
  File "${SRCROOT}\space-studio\src-tauri\dist\WebView2Loader.dll"
  File "${SRCROOT}\README.md"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateDirectory "$SMPROGRAMS\Space Studio"
  CreateShortCut "$SMPROGRAMS\Space Studio\Space Studio.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
  CreateShortCut "$DESKTOP\Space Studio.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0

  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\SpaceStudio" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\SpaceStudio" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\SpaceStudio" "Publisher" "Space OS"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\SpaceStudio" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\SpaceStudio" "UninstallString" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\WebView2Loader.dll"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$SMPROGRAMS\Space Studio\Space Studio.lnk"
  Delete "$DESKTOP\Space Studio.lnk"
  RMDir "$SMPROGRAMS\Space Studio"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\SpaceStudio"
  DeleteRegKey HKLM "Software\SpaceStudio"
SectionEnd
