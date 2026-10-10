"""python -m screencast.marker: an always-on-top live clock, to prove what the receiver shows is this desktop
(and to eyeball latency: compare the clock on the TV with the one on this screen)."""
import time
import tkinter as tk


def main():
    root = tk.Tk()
    root.title("open-airplay marker")
    root.attributes("-topmost", True)
    root.configure(bg="#ff00aa")
    w, h = 1100, 420
    root.geometry(f"{w}x{h}+{(root.winfo_screenwidth() - w) // 2}+{(root.winfo_screenheight() - h) // 2}")
    tk.Label(root, text="OPEN-AIRPLAY", fg="white", bg="#ff00aa", font=("Segoe UI", 64, "bold")).pack(pady=(30, 0))
    clock = tk.Label(root, fg="white", bg="#ff00aa", font=("Consolas", 96, "bold"))
    clock.pack()

    def tick():
        t = time.time()
        clock.config(text=time.strftime("%H:%M:%S", time.localtime(t)) + f".{int(t * 10) % 10}")
        root.after(50, tick)

    tick()
    root.mainloop()


if __name__ == "__main__":
    main()
