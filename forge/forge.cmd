@echo off
set "PYTHONPATH=%~dp0src"
if defined DRAMATIS_PACKS set "PYTHONPATH=%PYTHONPATH%;%DRAMATIS_PACKS%"
"%~dp0.venv\Scripts\python.exe" -m dramatis_forge %*
