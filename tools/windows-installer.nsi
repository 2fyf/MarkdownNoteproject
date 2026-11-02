Unicode true
RequestExecutionLevel user
!include "MUI2.nsh"
Name "MarkdownNote @VERSION@"
OutFile "@OUTFILE@"
InstallDir "$EXEDIR\MarkdownNote"
SetCompressor zlib
BrandingText "MarkdownNote"
Var InstallLock
Var RecordFile
Var PreviousPath
Var DistributionFolder
Var InstallerMutex
!define MUI_ICON "@ICON@"
!define MUI_UNICON "@ICON@"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TEXT "安装到您选择的可写文件夹，安装后双击 MarkdownNote.exe。重复安装会被阻止。卸载保留运行数据和日志。需要已有 Microsoft WebView2。"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Function .onInit
  StrCpy $RecordFile "$EXEDIR\MarkdownNote-install.ini"
  System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Local\MarkdownNoteInstaller") p.r0 ?e'
  Pop $1
  StrCpy $InstallerMutex $0
  IntCmp $1 183 0 init_continue init_continue
    MessageBox MB_OK|MB_ICONINFORMATION "已有安装或卸载程序正在运行，请等待其结束。" /SD IDOK
    SetErrorLevel 24
    Quit
  init_continue:
  ReadINIStr $PreviousPath $RecordFile "MarkdownNote" "Path"
  StrCmp $PreviousPath "" init_done
  IfFileExists "$PreviousPath\.markdownnote-app" 0 check_legacy
    MessageBox MB_OK|MB_ICONINFORMATION "已经安装 MarkdownNote。$\r$\n位置：$PreviousPath$\r$\n请直接打开该目录的 MarkdownNote.exe。重新安装前请先卸载，笔记数据会保留。" /SD IDOK
    SetErrorLevel 20
    Quit
  check_legacy:
  IfFileExists "$PreviousPath\应用\MarkdownNote.exe" 0 init_done
    MessageBox MB_OK|MB_ICONINFORMATION "已检测到旧版安装：$PreviousPath。请先退出并卸载旧版，再安装。" /SD IDOK
    SetErrorLevel 20
    Quit
  init_done:
FunctionEnd

Function VerifyTarget
  IfFileExists "$INSTDIR\运行数据\.MarkdownNote.running.lock" 0 check_version
    ClearErrors
    FileOpen $0 "$INSTDIR\运行数据\.MarkdownNote.running.lock" a
    IfErrors running
    FileClose $0
  check_version:
  IfFileExists "$INSTDIR\.markdown-notes-app" duplicate
  IfFileExists "$INSTDIR\MarkdownNotes.exe" duplicate
  IfFileExists "$INSTDIR\应用\MarkdownNotes.exe" duplicate
  IfFileExists "$INSTDIR\.markdownnote-app" duplicate
  IfFileExists "$INSTDIR\应用\MarkdownNote.exe" duplicate
  IfFileExists "$INSTDIR\MarkdownNote.exe" duplicate target_clear
  duplicate:
    MessageBox MB_OK|MB_ICONINFORMATION "目标目录已经安装程序：$INSTDIR$\r$\n本次不会覆盖。请直接启动；如需重新安装，请先退出并卸载。卸载保留笔记。" /SD IDOK
    SetErrorLevel 20
    Quit
  running:
    MessageBox MB_OK|MB_ICONEXCLAMATION "该目录的程序正在运行，或数据目录无法写入。请保存笔记并退出程序后重试。" /SD IDOK
    SetErrorLevel 23
    Quit
  target_clear:
FunctionEnd

Function .onVerifyInstDir
  Call VerifyTarget
FunctionEnd

Section "MarkdownNote"
  Call VerifyTarget
  CreateDirectory "$INSTDIR\运行数据"
  ClearErrors
  FileOpen $InstallLock "$INSTDIR\运行数据\.MarkdownNote.running.lock" a
  IfErrors install_lock_failed
  SetOverwrite on
@INSTALL@
  WriteUninstaller "$INSTDIR\卸载程序.exe"
  WriteINIStr "$INSTDIR\.markdownnote-install.ini" "MarkdownNote" "Version" "@VERSION@"
  WriteINIStr "$INSTDIR\.markdownnote-install.ini" "MarkdownNote" "DistributionFolder" "$EXEDIR"
  WriteINIStr $RecordFile "MarkdownNote" "Path" "$INSTDIR"
  WriteINIStr $RecordFile "MarkdownNote" "Version" "@VERSION@"
  IfErrors install_record_failed
  FileClose $InstallLock
  Goto install_done
  install_record_failed:
    FileClose $InstallLock
    MessageBox MB_OK|MB_ICONSTOP "无法保存安装记录。请将安装包放到有写入权限的文件夹后重试。程序文件保留在已选目录，可先运行卸载程序清理。" /SD IDOK
    SetErrorLevel 25
    Abort
  install_lock_failed:
    MessageBox MB_OK|MB_ICONSTOP "无法锁定目标目录，请确认程序已退出并检查目录权限。" /SD IDOK
    SetErrorLevel 23
    Abort
  install_done:
SectionEnd

Section "Uninstall"
  ClearErrors
  FileOpen $InstallLock "$INSTDIR\运行数据\.MarkdownNote.running.lock" a
  IfErrors uninstall_running
  ReadINIStr $DistributionFolder "$INSTDIR\.markdownnote-install.ini" "MarkdownNote" "DistributionFolder"
  StrCpy $RecordFile "$DistributionFolder\MarkdownNote-install.ini"
  ReadINIStr $PreviousPath $RecordFile "MarkdownNote" "Path"
  StrCmp $PreviousPath $INSTDIR 0 keep_registration
    Delete $RecordFile
  keep_registration:
@UNINSTALL@
  Delete "$INSTDIR\.markdownnote-install.ini"
  Delete "$INSTDIR\卸载程序.exe"
@RMDIRS@
  FileClose $InstallLock
  Delete "$INSTDIR\运行数据\.MarkdownNote.running.lock"
  RMDir "$INSTDIR\运行数据"
  RMDir "$INSTDIR"
  Goto uninstall_done
  uninstall_running:
    MessageBox MB_OK|MB_ICONEXCLAMATION "程序正在运行，或目录无法写入。请先保存并退出程序，再卸载。" /SD IDOK
    SetErrorLevel 23
    Quit
  uninstall_done:
SectionEnd
