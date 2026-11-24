def lstsq(rows, target, rcond=None):
    """Least squares by the normal equations (Gauss-Jordan with pivoting)."""
    cols = [c.v for c in rows.cols]
    t = target.v
    n = len(cols)
    A = [[sum(a * b for a, b in zip(cols[i], cols[j])) for j in range(n)] for i in range(n)]
    y = [sum(a * b for a, b in zip(cols[i], t)) for i in range(n)]
    M = [A[i] + [y[i]] for i in range(n)]
    for k in range(n):
        p = max(range(k, n), key=lambda r: abs(M[r][k]))
        M[k], M[p] = M[p], M[k]
        piv = M[k][k]
        if piv == 0:
            continue
        M[k] = [v / piv for v in M[k]]
        for r in range(n):
            if r != k and M[r][k] != 0:
                f = M[r][k]
                M[r] = [a - f * b for a, b in zip(M[r], M[k])]
    return ([M[i][n] for i in range(n)], None, None, None)
