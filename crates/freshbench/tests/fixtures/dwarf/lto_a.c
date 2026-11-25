struct pt { int x; long y; };
typedef unsigned char u8;
__attribute__((noinline)) double scale(const struct pt *p, u8 k)
{
    volatile int t = p->x * k;
    return t * 1.5 + (double)p->y;
}
