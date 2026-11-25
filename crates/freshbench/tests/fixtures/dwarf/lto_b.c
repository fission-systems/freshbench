struct pt { int x; long y; };
double scale(const struct pt *p, unsigned char k);
int main(int argc, char **argv)
{
    struct pt p = { argc, 7 };
    return (int)scale(&p, (unsigned char)argc);
}
