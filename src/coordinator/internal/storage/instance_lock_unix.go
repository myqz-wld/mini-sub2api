//go:build darwin || linux

package storage

import (
	"fmt"
	"os"
	"path/filepath"
	"syscall"
)

type ServiceLock struct {
	file *os.File
}

func AcquireServiceLock(stateDir string) (*ServiceLock, error) {
	path := filepath.Join(stateDir, "coordinator-instance.lock")
	file, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, fmt.Errorf("open coordinator instance lock: %w", err)
	}
	if err := file.Chmod(0o600); err != nil {
		file.Close()
		return nil, fmt.Errorf("protect coordinator instance lock: %w", err)
	}
	if err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		file.Close()
		return nil, fmt.Errorf("another mini-sub2api server is using this state directory")
	}
	return &ServiceLock{file: file}, nil
}

func (l *ServiceLock) Close() error {
	if l == nil || l.file == nil {
		return nil
	}
	unlockErr := syscall.Flock(int(l.file.Fd()), syscall.LOCK_UN)
	closeErr := l.file.Close()
	l.file = nil
	if unlockErr != nil {
		return unlockErr
	}
	return closeErr
}
