' Huaxia build launcher - double click this file
Set sh = CreateObject("WScript.Shell")
sh.CurrentDirectory = "D:\Qoderwork"
ret = sh.Run("cmd.exe /k D:\Qoderwork\build-go.cmd", 1, False)
