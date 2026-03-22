@echo off
setlocal enableextensions

set "VCVARS="

for %%P in (
	"C:\Program Files\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat"
	"C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvars64.bat"
	"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
	"C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
	"C:\Program Files\Microsoft Visual Studio\2022\Preview\VC\Auxiliary\Build\vcvars64.bat"
) do (
	if exist %%~P (
		set "VCVARS=%%~P"
		goto :found
	)
)

if exist "%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" (
	for /f "usebackq delims=" %%I in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do (
		if exist "%%~I\VC\Auxiliary\Build\vcvars64.bat" (
			set "VCVARS=%%~I\VC\Auxiliary\Build\vcvars64.bat"
			goto :found
		)
	)
)

echo Failed to locate vcvars64.bat. Install Visual Studio C++ build tools or update .vscode\cargo_with_vcvars.cmd.
exit /b 1

:found
call "%VCVARS%"
if errorlevel 1 exit /b %errorlevel%

cargo %*
