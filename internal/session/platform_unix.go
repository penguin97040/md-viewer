//go:build !windows

package session

import (
	"golang.org/x/sys/unix"
	"os"
	"os/exec"
)

func protect(path string) error {
	st, e := os.Stat(path)
	if e != nil {
		return e
	}
	mode := os.FileMode(0600)
	if st.IsDir() {
		mode = 0700
	}
	return os.Chmod(path, mode)
}
func lockFile(path string) (*os.File, error) {
	f, e := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0600)
	if e != nil {
		return nil, e
	}
	if e = unix.Flock(int(f.Fd()), unix.LOCK_EX|unix.LOCK_NB); e != nil {
		f.Close()
		return nil, e
	}
	return f, nil
}
func prepareChild(cmd *exec.Cmd, _ bool) { cmd.SysProcAttr = &unix.SysProcAttr{Setsid: true} }
